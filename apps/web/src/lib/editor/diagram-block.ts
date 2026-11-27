/**
 * A code block in the editor, and the diagram a block in the language of
 * diagrams is.
 *
 * A diagram shows its picture and nothing else until its place is opened
 * (editing-place.ts). Open, the picture stays on top and the source is edited
 * right under it, as the text of the block: typing, undo and input methods
 * are the editor's own. The picture is drawn again a moment after the source
 * last changed; the one on screen stays until the next is ready, and a
 * source that cannot be drawn leaves the last picture dimmed, with what went
 * wrong said under the source.
 *
 * Closed, a diagram that cannot be drawn shows its source as text and what
 * went wrong in the place of the picture, and an empty one says it is empty.
 *
 * Nothing here is part of the document: the node is the code block it always
 * was (`renderHTML` of the extension), and the picture is drawn from its text.
 * A code block in any other language is code-block-view.ts, coloured by
 * code-highlight.ts.
 */
import { CodeBlock } from '@tiptap/extension-code-block';
import type { Node as DocumentNode } from '@tiptap/pm/model';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import type { Decoration, EditorView, NodeView, ViewMutationRecord } from '@tiptap/pm/view';

import { m } from '$lib/i18n/paraglide/messages.js';

import { contentWasSplit } from './browser-edits.ts';
import { CodeBlockView } from './code-block-view.ts';
import { codeHighlight } from './code-highlight.ts';
import type { DiagramResult } from './diagram.ts';
import { diagramKind } from './diagram-kind.ts';
import { DIAGRAM_REDRAW_DELAY, diagramProblem, onLookChange } from './embeds.ts';
import {
  closePlace,
  closesPlace,
  isDiagramNode,
  openPlace,
  placeEditButton,
  placeFooter,
  placeIn,
} from './editing-place.ts';
import { isLanguage } from './schema.ts';

type Shown = 'pending' | 'drawn' | 'error' | 'empty';
type DiagramFailed = Extract<DiagramResult, { ok: false }>;

function element<K extends keyof HTMLElementTagNameMap>(
  page: Document,
  tag: K,
  attributes: Record<string, string> = {},
): HTMLElementTagNameMap[K] {
  const made = page.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) made.setAttribute(name, value);
  return made;
}

let lastId = 0;

/** Whether two failures say the same thing (two drawings never give the same object). */
const sameFailure = (one: DiagramFailed | undefined, other: DiagramFailed | undefined): boolean =>
  one?.reason === other?.reason && one?.detail === other?.detail;

/**
 * The last picture drawn for an open diagram's editing place, keyed by the
 * place's anchor object so it survives ProseMirror rebuilding a dirty or
 * remotely replaced node view and is collected once the place closes.
 */
interface KeptDiagram {
  picture: SVGSVGElement;
  source: string;
  look: string | null;
}
const keptPictures = new WeakMap<object, KeptDiagram>();

class DiagramView implements NodeView {
  readonly dom: HTMLElement;
  readonly contentDOM: HTMLElement;
  readonly #page: Document;
  readonly #view: EditorView;
  readonly #getPos: () => number | undefined;
  readonly #figure: HTMLElement;
  readonly #problem: HTMLElement;
  readonly #stopWatching: () => void;
  #source: string;
  #open = false;
  #anchor: object | undefined;
  #shown: Shown = 'pending';
  #picture: SVGSVGElement | undefined;
  #drawnSource: string | undefined;
  #drawnLook: string | null | undefined;
  #failure: DiagramFailed | undefined;
  #timer: ReturnType<typeof setTimeout> | undefined;
  /** The source being drawn now, if a drawing is under way. */
  #drawing: string | undefined;
  /** Asked to draw again while a drawing was under way (the page turned dark, say). */
  #again = false;
  /** The failure the line under the source says, so that it is said once. */
  #said: DiagramFailed | undefined;
  #destroyed = false;
  /** Names the parts that others point to (`aria-describedby`). */
  readonly #id = `doc-diagram-block-${(lastId += 1)}`;
  /** The diagrams of the same editor, this one among them while it is in the page. */
  readonly #known: Set<DiagramView>;

  constructor(
    node: DocumentNode,
    view: EditorView,
    getPos: () => number | undefined,
    decorations: readonly Decoration[],
    known: Set<DiagramView>,
  ) {
    this.#known = known.add(this);
    this.#page = view.dom.ownerDocument;
    this.#view = view;
    this.#getPos = getPos;
    this.#source = node.textContent;
    const place = placeIn(decorations);
    this.#open = place !== undefined;
    this.#anchor = place?.anchor;
    const kept = this.#anchor ? keptPictures.get(this.#anchor) : undefined;
    if (kept) {
      this.#picture = kept.picture;
      this.#drawnSource = kept.source;
      this.#drawnLook = kept.look;
      this.#shown = this.#source.trim() === '' ? 'empty' : 'drawn';
    }
    const page = this.#page;

    this.dom = element(page, 'div', { class: 'doc-diagram-block' });
    this.#figure = element(page, 'div', { class: 'doc-diagram', contenteditable: 'false' });
    const source = element(page, 'pre', { class: 'doc-diagram-source' });
    this.contentDOM = element(page, 'code', { class: 'language-mermaid' });
    source.append(this.contentDOM);
    this.#problem = element(page, 'div', {
      class: 'doc-diagram-problem',
      contenteditable: 'false',
      'aria-live': 'polite',
    });
    const footer = placeFooter(page, () => {
      closePlace(this.#view, true);
    });
    // The keys that close the place work on its button too, which the editor does not hear.
    footer.addEventListener('keydown', (event) => {
      if (closesPlace(event)) {
        event.preventDefault();
        event.stopPropagation();
        closePlace(this.#view, true);
      }
    });
    this.dom.append(
      this.#figure,
      placeEditButton(page, () => {
        this.#press();
      }),
      source,
      this.#problem,
      footer,
    );

    this.#figure.addEventListener('click', (event) => {
      event.preventDefault();
      this.#press();
    });
    this.#stopWatching = onLookChange(page, () => {
      this.#schedule(0);
    });
    this.refresh();
    const look = page.documentElement.getAttribute('class');
    if (kept?.look !== look) this.#schedule(0);
    else if (kept.source !== this.#source) this.#schedule(DIAGRAM_REDRAW_DELAY);
  }

  /** A press on the picture or its button: the place opens. */
  #press(): void {
    const pos = this.#getPos();
    if (pos !== undefined && !this.#open) openPlace(this.#view, pos);
  }

  #rememberPicture(): void {
    if (
      this.#anchor &&
      this.#picture &&
      this.#drawnSource !== undefined &&
      this.#drawnLook !== undefined
    ) {
      keptPictures.set(this.#anchor, {
        picture: this.#picture,
        source: this.#drawnSource,
        look: this.#drawnLook,
      });
    }
  }

  update(node: DocumentNode, decorations: readonly Decoration[]): boolean {
    if (!isDiagramNode(node)) return false;
    const place = placeIn(decorations);
    const open = place !== undefined;
    const source = node.textContent;
    const changed = source !== this.#source;
    if (place?.anchor !== this.#anchor) {
      this.#anchor = place?.anchor;
      this.#rememberPicture();
    }
    if (!changed && open === this.#open) return true;
    this.#source = source;
    this.#open = open;
    if (changed) this.#schedule(DIAGRAM_REDRAW_DELAY);
    this.refresh();
    return true;
  }

  /** What is pressed on the picture, its button or the line under the source is not the editor's. */
  stopEvent(event: Event): boolean {
    return event.target instanceof Node && !this.contentDOM.contains(event.target);
  }

  ignoreMutation(mutation: ViewMutationRecord): boolean {
    if (mutation.type === 'selection') return false;
    if (contentWasSplit(mutation, this.contentDOM)) return false;
    return !this.contentDOM.contains(mutation.target);
  }

  destroy(): void {
    this.#destroyed = true;
    this.#known.delete(this);
    clearTimeout(this.#timer);
    this.#stopWatching();
  }

  #schedule(wait: number): void {
    clearTimeout(this.#timer);
    if (this.#source.trim() === '') {
      this.#shown = 'empty';
      this.refresh();
      return;
    }
    this.#timer = setTimeout(() => {
      void this.#draw();
    }, wait);
  }

  /** Draws the source as it is now. One drawing at a time; the newest source is drawn next. */
  async #draw(): Promise<void> {
    if (this.#drawing !== undefined) {
      this.#again = true;
      return;
    }
    const source = this.#source;
    const look = this.#page.documentElement.getAttribute('class');
    this.#drawing = source;
    this.#again = false;
    let result: DiagramResult;
    try {
      const { drawDiagram } = await import('./diagram.ts');
      const width = this.dom.clientWidth || this.dom.parentElement?.clientWidth || 0;
      result = await drawDiagram(source, width, this.#page);
    } catch (error) {
      result = { ok: false, reason: 'failed', detail: error instanceof Error ? error.message : '' };
    }
    this.#drawing = undefined;
    if (this.#destroyed) return;
    if (this.#isOld(source)) {
      // Typed on meanwhile, or the look changed: what was drawn is already old.
      this.#schedule(source === this.#source ? 0 : DIAGRAM_REDRAW_DELAY);
      return;
    }
    if (result.ok) {
      this.#picture = result.picture;
      this.#drawnSource = source;
      this.#drawnLook = look;
      this.#failure = undefined;
      this.#shown = 'drawn';
      this.#rememberPicture();
    } else {
      this.#failure = result;
      this.#shown = 'error';
    }
    this.refresh();
  }

  /** Whether a drawing of `source` that has just ended is out of date already. */
  #isOld(source: string): boolean {
    return source !== this.#source || this.#again;
  }

  /** Brings the page up to date with the source, the drawing and whether the place is open. */
  refresh(): void {
    const page = this.#page;
    const figure = this.#figure;
    const shown = this.#shown;
    this.dom.toggleAttribute('data-open', this.#open);
    this.dom.setAttribute('data-state', shown);
    figure.setAttribute('data-state', shown);
    figure.setAttribute('aria-busy', String(shown === 'pending'));
    // Open, a picture that cannot be drawn again stays, dimmed, until one can.
    const stale = this.#open && shown === 'error' && this.#picture !== undefined;
    figure.toggleAttribute('data-stale', stale);
    const pictured = shown === 'drawn' || stale;
    const failure = shown === 'error' ? this.#failure : undefined;

    if (pictured) {
      if (this.#picture && figure.firstChild !== this.#picture)
        figure.replaceChildren(this.#picture);
    } else if (this.#open || shown === 'pending') {
      // Nothing to show yet, or nothing while the source is in sight under it.
      figure.replaceChildren();
    } else if (shown === 'empty') {
      const empty = element(page, 'p', { class: 'doc-diagram-empty' });
      empty.append(page.createTextNode(m.editor_diagram_empty()));
      figure.replaceChildren(empty);
    } else {
      // Closed and not drawn: the source, readable, in place of the picture.
      const copy = element(page, 'pre', { class: 'doc-diagram-copy' });
      copy.append(page.createTextNode(this.#source));
      figure.replaceChildren(...(failure ? diagramProblem(page, failure) : []), copy);
    }
    this.#name(pictured, failure);

    // Said once for each failure, not again with every key typed or every drawing that fails
    // the same way meanwhile: the words in the live region change only when the words do.
    const problem = this.#open ? failure : undefined;
    if (!sameFailure(problem, this.#said)) {
      this.#said = problem;
      this.#problem.replaceChildren(...(problem ? diagramProblem(page, problem) : []));
    }
  }

  /** The name of the picture: its kind, what a press does, and what went wrong with it. */
  #name(pictured: boolean, failure: DiagramFailed | undefined): void {
    const figure = this.#figure;
    const kind = this.#shown === 'empty' ? m.editor_diagram_empty() : diagramKind(this.#source);
    if (this.#view.editable && !this.#open) {
      figure.setAttribute('role', 'button');
      figure.setAttribute('aria-label', m.editor_diagram_named({ kind }));
    } else {
      figure.setAttribute('role', pictured ? 'img' : 'group');
      figure.setAttribute('aria-label', kind);
    }
    // A button's label is all that is read of it: what went wrong is its description.
    const notice = failure && !this.#open ? figure.querySelector('[role="note"]') : null;
    if (notice) {
      notice.id = `${this.#id}-problem`;
      figure.setAttribute('aria-describedby', notice.id);
    } else figure.removeAttribute('aria-describedby');
  }
}

/** The diagrams of each editor, to be told when it turns editable or read-only. */
const diagramsOf = new WeakMap<EditorView, Set<DiagramView>>();

/** The code block of the body: plain, or a diagram when its language is that of diagrams. */
export const BodyCodeBlock = CodeBlock.extend({
  addAttributes() {
    const parent = (this.parent?.() ?? {}) as Record<string, Record<string, unknown> | undefined>;
    return {
      ...parent,
      language: {
        ...parent.language,
        default: null,
        parseHTML: (element: HTMLElement) => {
          const prefix = this.options.languageClassPrefix ?? 'language-';
          const classes = [...(element.firstElementChild?.classList ?? [])];
          const found = classes
            .filter((name) => name.startsWith(prefix))
            .map((name) => name.slice(prefix.length))[0];
          return found && isLanguage(found) ? found : null;
        },
      },
    };
  },

  addNodeView() {
    return ({ node, view, getPos, decorations }) => {
      if (!isDiagramNode(node)) return new CodeBlockView(node, view, getPos);
      const known = diagramsOf.get(view) ?? new Set<DiagramView>();
      diagramsOf.set(view, known);
      return new DiagramView(node, view, getPos, decorations, known);
    };
  },

  addProseMirrorPlugins() {
    return [
      codeHighlight(),
      new Plugin({
        key: new PluginKey('diagramEditable'),
        view: (view) => {
          // The editor may have been told it is read-only only after its first drawing.
          let editable = view.editable;
          for (const diagram of diagramsOf.get(view) ?? []) diagram.refresh();
          return {
            update: () => {
              if (view.editable === editable) return;
              editable = view.editable;
              for (const diagram of diagramsOf.get(view) ?? []) diagram.refresh();
            },
          };
        },
      }),
    ];
  },
});
