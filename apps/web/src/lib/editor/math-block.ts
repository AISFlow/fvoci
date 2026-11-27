/**
 * A formula on a line of its own, in the editor: its drawing, and the place
 * where its source is changed, opened right under the drawing in the flow of
 * the body (editing-place.ts), the way a diagram's is.
 *
 * Open, the drawing above follows the field as it is typed in; a source that
 * cannot be drawn leaves the last drawing dimmed, with what went wrong said
 * under the field. What the field holds goes into the document when the
 * place is closed on purpose (a key, its button, a press or the focus
 * elsewhere), as one step of the body's undo; a formula closed empty is
 * removed (editing-place.ts). Until then the platform's undo key is the field's own.
 *
 * A new formula (`$$` and a space on an empty line) has no node yet: a
 * formula without a source is none. Its place is drawn on that empty line,
 * with a preview above the field, and the formula goes in when the place
 * closes with something written.
 */
import type { Editor } from '@tiptap/core';
import type { Node as DocumentNode } from '@tiptap/pm/model';
import type { Decoration, NodeView } from '@tiptap/pm/view';

import { shortcutLabel } from '$lib/keys.ts';
import { m } from '$lib/i18n/paraglide/messages.js';

import { MATH_SOURCE_ATTRIBUTE, mathSlotAttributes } from './drawing.ts';
import {
  closePlace,
  closesPlace,
  openPlace,
  placeEditButton,
  placeFooter,
  placeIn,
  type NewFormulaPanel,
  type Place,
} from './editing-place.ts';
import { fillMath, mathProblem } from './embeds.ts';
import { MAX_LATEX } from './schema.ts';

const latexOf = (node: DocumentNode): string =>
  typeof node.attrs.latex === 'string' ? node.attrs.latex : '';

/** A fresh element for a block formula, holding its source until it is drawn. */
function formulaSlot(page: Document, latex: string): HTMLElement {
  const slot = page.createElement('div');
  for (const [name, value] of Object.entries(mathSlotAttributes('blockMath', latex)))
    slot.setAttribute(name, value);
  slot.append(page.createTextNode(latex));
  return slot;
}

/**
 * Draws a formula out of sight, inside `beside`, and gives the element it was
 * drawn into and whether it could be. Out of sight but laid out, so that a
 * formula far too tall is still caught.
 */
async function drawAside(
  beside: HTMLElement,
  latex: string,
): Promise<{ slot: HTMLElement; ok: boolean }> {
  const page = beside.ownerDocument;
  const slot = formulaSlot(page, latex);
  const holder = page.createElement('div');
  holder.setAttribute('class', 'doc-math-aside');
  holder.setAttribute('aria-hidden', 'true');
  holder.append(slot);
  beside.append(holder);
  await fillMath(slot).catch(() => undefined);
  holder.remove();
  return { slot, ok: slot.getAttribute('data-state') === 'drawn' };
}

/** The field of a place. */
function sourceField(page: Document, latex: string): HTMLTextAreaElement {
  const field = page.createElement('textarea');
  field.setAttribute('aria-label', m.editor_math_source());
  field.setAttribute('maxlength', String(MAX_LATEX));
  field.setAttribute('rows', '3');
  field.setAttribute('spellcheck', 'false');
  field.setAttribute('autocapitalize', 'off');
  field.setAttribute('autocomplete', 'off');
  field.value = latex;
  return field;
}

/** Says what went wrong under the field; the same words again are not said again. */
function say(problem: HTMLElement, parts: HTMLElement[]): void {
  const words = parts.map((part) => part.textContent).join(' ');
  if (problem.textContent !== words) problem.replaceChildren(...parts);
}

/** The box under a formula: the field, what went wrong (said aloud), how to close. */
function placeBox(page: Document, editor: Editor, field: HTMLTextAreaElement) {
  const problem = page.createElement('div');
  problem.setAttribute('class', 'doc-place-problem');
  problem.setAttribute('aria-live', 'polite');
  const box = page.createElement('div');
  box.setAttribute('class', 'doc-place');
  box.setAttribute('contenteditable', 'false');
  box.append(
    field,
    problem,
    placeFooter(
      page,
      () => {
        closePlace(editor.view, true);
      },
      m.editor_math_hint_block({ shortcut: shortcutLabel('Enter') }),
    ),
  );
  // The keys that close the place work anywhere in it: the editor does not hear them here.
  box.addEventListener('keydown', (event) => {
    if (!closesPlace(event)) return;
    event.preventDefault();
    event.stopPropagation();
    closePlace(editor.view, true);
  });
  return { box, problem };
}

let lastId = 0;

class FormulaView implements NodeView {
  readonly dom: HTMLElement;
  readonly #editor: Editor;
  readonly #getPos: () => number | undefined;
  #slot: HTMLElement;
  readonly #hint: HTMLElement;
  #latex: string;
  #place: Place | undefined;
  #panel:
    | {
        box: HTMLElement;
        field: HTMLTextAreaElement;
        problem: HTMLElement;
        /** Whether the field was written in since the place opened. */
        written: () => boolean;
      }
    | undefined;
  /** Counts drawings, so that only the newest one is shown. */
  #drawn = 0;

  constructor(
    node: DocumentNode,
    editor: Editor,
    getPos: () => number | undefined,
    decorations: readonly Decoration[],
    page: Document,
  ) {
    this.#editor = editor;
    this.#getPos = getPos;
    this.#latex = latexOf(node);
    this.dom = page.createElement('div');
    this.dom.setAttribute('class', 'doc-math-block');
    this.#slot = formulaSlot(page, this.#latex);
    // For the eye, the same press as on the formula; for the ear, its description says so.
    const hint = page.createElement('span');
    hint.id = `doc-math-block-${(lastId += 1)}-hint`;
    hint.setAttribute('class', 'doc-announce');
    hint.append(page.createTextNode(m.editor_math_press()));
    this.#hint = hint;
    this.dom.append(this.#slot, placeEditButton(page, this.#open), hint);
    this.#show();
    this.#setPlace(placeIn(decorations));
  }

  readonly #open = (): void => {
    const pos = this.#getPos();
    if (pos !== undefined && !this.#place) openPlace(this.#editor.view, pos);
  };

  /** Puts in a new element for the drawing, in place of the one there. */
  #replaceSlot(slot: HTMLElement): void {
    slot.addEventListener('click', this.#open);
    if (this.#editor.isEditable) slot.setAttribute('aria-describedby', this.#hint.id);
    this.#slot.replaceWith(slot);
    this.#slot = slot;
  }

  /** Whether the drawing shown is of `latex`, finished and not dimmed. */
  #shows(latex: string): boolean {
    const slot = this.#slot;
    return (
      slot.getAttribute(MATH_SOURCE_ATTRIBUTE) === latex &&
      slot.getAttribute('data-state') === 'drawn' &&
      !slot.hasAttribute('data-stale')
    );
  }

  /** Draws the stored source the way a closed formula shows it: a failure shows the source. */
  #show(): void {
    this.#drawn += 1;
    this.#replaceSlot(formulaSlot(this.dom.ownerDocument, this.#latex));
    // Without the library the source stays, which is what a failure looks like anyway.
    fillMath(this.#slot).catch(() => undefined);
  }

  /** While the place is open: the new drawing when it can be drawn, else the last one, dimmed. */
  async #preview(latex: string): Promise<void> {
    this.#drawn += 1;
    const mine = this.#drawn;
    const { slot, ok } = await drawAside(this.dom, latex);
    if (mine !== this.#drawn || !this.#panel) return;
    const { problem } = this.#panel;
    if (ok) {
      this.#replaceSlot(slot);
      problem.replaceChildren();
    } else {
      this.#slot.toggleAttribute('data-stale', this.#slot.getAttribute('data-state') === 'drawn');
      say(problem, mathProblem(this.dom.ownerDocument, slot));
    }
  }

  #setPlace(place: Place | undefined): void {
    this.#place = place;
    if (place && !this.#panel) {
      const page = this.dom.ownerDocument;
      // A place already open (this view was made again) keeps what its field held.
      const held = place.draft?.touched === true ? place.draft.value() : this.#latex;
      const field = sourceField(page, held);
      const { box, problem } = placeBox(page, this.#editor, field);
      const touched = (): boolean => place.draft?.touched === true;
      field.addEventListener('input', () => {
        if (place.draft) place.draft.touched = true;
        this.#edited(field.value);
      });
      this.dom.append(box);
      this.#panel = { box, field, problem, written: touched };
      this.dom.setAttribute('data-open', '');
      // What is wrong with the source is said under the field from the start.
      void this.#preview(held);
    } else if (!place && this.#panel) {
      this.#panel.box.remove();
      this.#panel = undefined;
      this.dom.removeAttribute('data-open');
      // The last drawing of the place is often the stored source already: it stays. Drawn
      // again, the block would change under a selection that has just come back to it, and
      // WebKit then moves that selection to the next line.
      if (!this.#shows(this.#latex)) this.#show();
      else this.#drawn += 1;
    }
    if (place?.draft && this.#panel) {
      const { field, written } = this.#panel;
      // What closing the place keeps: the field once it was written in (it may be empty while
      // the document is not); until then the source as it is stored now, exactly, so that only
      // opening and closing changes nothing, even when the source changed meanwhile elsewhere.
      place.draft.value = () => (written() ? field.value.trim().slice(0, MAX_LATEX) : this.#latex);
    }
  }

  /** A change of the field: drawn above it, and kept for when the place closes. */
  #edited(value: string): void {
    const latex = value.trim().slice(0, MAX_LATEX);
    if (latex === '') {
      // Nothing to draw: the last drawing stays, dimmed, until the field says more.
      this.#drawn += 1;
      this.#slot.toggleAttribute('data-stale', true);
      this.#panel?.problem.replaceChildren();
      return;
    }
    void this.#preview(latex);
  }

  update(node: DocumentNode, decorations: readonly Decoration[]): boolean {
    if (node.type.name !== 'blockMath') return false;
    const latex = latexOf(node);
    const before = this.#latex;
    const changed = latex !== before;
    this.#latex = latex;
    const place = placeIn(decorations);
    // Closing draws the stored source again by itself.
    const closing = place === undefined && this.#place !== undefined;
    if (place !== this.#place) this.#setPlace(place);
    if (changed && this.#panel) {
      // Changed from elsewhere while open: the field follows, unless it was written in.
      const { field, written } = this.#panel;
      if (!written() && field.value === before) {
        field.value = latex;
        void this.#preview(latex);
      }
    } else if (changed && !closing) this.#show();
    return true;
  }

  /** What happens in the box under the formula is not the editor's. */
  stopEvent(event: Event): boolean {
    return event.target instanceof Node && this.#panel?.box.contains(event.target) === true;
  }

  ignoreMutation(): boolean {
    // What changes inside is the drawing and the field, never the document.
    return true;
  }
}

/** The node view of a block formula. */
export function formulaBlockView(props: {
  node: DocumentNode;
  editor: Editor;
  getPos: () => number | undefined;
  decorations: readonly Decoration[];
}): NodeView {
  const { node, editor, getPos, decorations } = props;
  // The page the editor is in. (Its view is not there yet while the first nodes are drawn.)
  return new FormulaView(node, editor, getPos, decorations, document);
}

/** The place of a new formula, drawn on its empty line until the formula goes in. */
export function newFormulaPanel(editor: Editor): NewFormulaPanel {
  const page = editor.view.dom.ownerDocument;
  const preview = page.createElement('div');
  preview.setAttribute('class', 'doc-math-preview');
  const field = sourceField(page, '');
  const { box, problem } = placeBox(page, editor, field);
  box.classList.add('doc-place-new');
  box.prepend(preview);
  let drawn = 0;
  field.addEventListener('input', () => {
    const latex = field.value.trim();
    drawn += 1;
    const mine = drawn;
    if (latex === '') {
      preview.replaceChildren();
      problem.replaceChildren();
      return;
    }
    void drawAside(box, latex).then(({ slot, ok }) => {
      if (mine !== drawn) return;
      if (ok) {
        preview.replaceChildren(slot);
        problem.replaceChildren();
      } else {
        preview.firstElementChild?.setAttribute('data-stale', '');
        say(problem, mathProblem(page, slot));
      }
    });
  });
  return {
    dom: box,
    value: () => field.value.trim().slice(0, MAX_LATEX),
    focus: () => {
      field.focus();
    },
  };
}
