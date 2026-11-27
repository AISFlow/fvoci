/**
 * A code block in the editor: the block's text in `<pre><code>`, as stored
 * (`renderHTML` of the extension), under a bar with the button that names
 * and changes its language and the button that copies its text. The bar is
 * no part of the document or of the text; what is pressed or typed in it is
 * not the editor's. The colours and the numbers of the lines are
 * decorations (code-highlight.ts).
 *
 * A diagram is another view (diagram-block.ts).
 */
import type { Node as DocumentNode } from '@tiptap/pm/model';
import { TextSelection } from '@tiptap/pm/state';
import type { EditorView, NodeView, ViewMutationRecord } from '@tiptap/pm/view';

import { contentWasSplit } from './browser-edits.ts';
import './code-block.css';
import { copyControls, type CopyControls } from './code-copy.ts';
import { CODE_COPY_TEXTS } from './code-copy-texts.ts';
import { LanguagePicker } from './code-language-picker.ts';
import { lineCount, lineDigits } from './code-lines.ts';
import { codeBar, codeFrame, lineColumn, numberColumn, numberedInColumn } from './code-paint.ts';
import { DIAGRAM_LANGUAGE } from './schema.ts';

/** The controls of a code block's bar, which the focus may be on: the editor takes no typing then. */
export const CODE_CONTROLS = 'button.doc-code-language, button.doc-code-copy';

const storedLanguage = (node: DocumentNode): string | null =>
  typeof node.attrs.language === 'string' ? node.attrs.language : null;

/** The code blocks of each editor, to be told when it turns read-only or editable. */
const blocksOf = new WeakMap<EditorView, Set<CodeBlockView>>();

/** Tells the code blocks of an editor that it turned editable or read-only. */
export function codeBlocksEditable(view: EditorView): void {
  for (const block of blocksOf.get(view) ?? []) block.editable(view.editable);
}

export class CodeBlockView implements NodeView {
  readonly dom: HTMLElement;
  readonly contentDOM: HTMLElement;
  readonly #view: EditorView;
  readonly #getPos: () => number | undefined;
  readonly #bar: HTMLElement;
  readonly #picker: LanguagePicker;
  readonly #copy: CopyControls;
  /** The numbers of a long block's lines, beside its text (code-paint.ts). */
  readonly #column: HTMLElement;
  #lines = 0;
  #node: DocumentNode;

  constructor(node: DocumentNode, view: EditorView, getPos: () => number | undefined) {
    const page = view.dom.ownerDocument;
    this.#node = node;
    this.#view = view;
    this.#getPos = getPos;
    const pre = page.createElement('pre');
    this.contentDOM = page.createElement('code');
    pre.append(this.contentDOM);
    this.#picker = new LanguagePicker(page, storedLanguage(node), {
      editable: () => view.editable,
      choose: (language) => {
        this.#choose(language);
      },
    });
    this.#copy = copyControls(page, {
      texts: CODE_COPY_TEXTS,
      text: () => this.#node.textContent,
      select: () => {
        this.#selectText();
      },
    });
    this.#bar = codeBar(page, this.#picker.element, this.#copy.status, this.#copy.button);
    this.dom = codeFrame(page, this.#bar, pre);
    this.#column = lineColumn(page);
    this.dom.append(this.#column);
    this.#draw(node);
    this.editable(view.editable);
    const known = blocksOf.get(view) ?? new Set<CodeBlockView>();
    blocksOf.set(view, known.add(this));
  }

  /** Brings the parts that are not text in step with the block. */
  #draw(node: DocumentNode): void {
    const language = storedLanguage(node);
    if (language) this.contentDOM.setAttribute('class', `language-${language}`);
    else this.contentDOM.removeAttribute('class');
    this.#picker.set(language);
    const lines = lineCount(node.textContent);
    this.dom.setAttribute('data-line-digits', String(lineDigits(lines)));
    // Numbered in one column, while numbers are shown: its lines do not wrap (code-block.css).
    const long = numberedInColumn(lines, node.textContent.length);
    this.dom.toggleAttribute('data-long', long);
    const numbered = long ? lines : 0;
    if (numbered !== this.#lines) numberColumn(this.#column, numbered);
    this.#lines = numbered;
  }

  /** The editor turned editable or read-only: the language can be changed, or only read. */
  editable(editable: boolean): void {
    this.dom.toggleAttribute('data-readonly', !editable);
    this.#picker.enable(editable);
  }

  #choose(language: string | null): void {
    const pos = this.#getPos();
    if (pos === undefined || this.#view.state.doc.nodeAt(pos) !== this.#node) return;
    this.#view.dispatch(this.#view.state.tr.setNodeAttribute(pos, 'language', language));
  }

  /** Selects the block's text: what the browser copies, or the reader does with the keys. */
  #selectText(): void {
    const pos = this.#getPos();
    if (pos === undefined) return;
    const { state } = this.#view;
    const end = pos + 1 + this.#node.content.size;
    this.#view.dispatch(state.tr.setSelection(TextSelection.create(state.doc, pos + 1, end)));
    this.#view.focus();
  }

  update(node: DocumentNode): boolean {
    if (node.type !== this.#node.type || node.attrs.language === DIAGRAM_LANGUAGE) return false;
    if (node.attrs.language !== this.#node.attrs.language || node.content !== this.#node.content)
      this.#draw(node);
    this.#node = node;
    return true;
  }

  /** What is pressed or typed in the bar is the bar's. */
  stopEvent(event: Event): boolean {
    return event.target instanceof Node && this.#bar.contains(event.target);
  }

  ignoreMutation(mutation: ViewMutationRecord): boolean {
    // The focus in the list's field is no selection of the text.
    if (mutation.type === 'selection') return this.#bar.contains(mutation.target);
    if (contentWasSplit(mutation, this.contentDOM)) return false;
    return !this.contentDOM.contains(mutation.target);
  }

  destroy(): void {
    this.#picker.destroy();
    this.#copy.destroy();
    blocksOf.get(this.#view)?.delete(this);
  }
}
