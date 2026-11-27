/**
 * The small box in which the source of a formula in a line is changed. (A
 * block formula is changed in its place under it: math-block.ts.)
 *
 * It opens under the formula with the source in a text field and the drawn
 * formula below it, redrawn as the source changes; a source that cannot be
 * drawn shows why instead. Plain DOM, styled by editor.css: this layer cannot
 * use the app's popover.
 *
 * Enter applies, and so does moving on to something else. Escape gives up
 * the change. Applying an empty source removes the formula.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

import { MATH_CLASS, MATH_SLOTS, MATH_SOURCE_ATTRIBUTE } from './drawing.ts';
import { fillMath } from './embeds.ts';
import { MAX_LATEX } from './schema.ts';

export interface MathEditingOptions {
  /** The source to start from. */
  latex: string;
  /** Where on the screen the formula is, now. Asked again when the window changes. */
  anchor: () => { left: number; top: number; bottom: number } | undefined;
  /**
   * The source to keep; empty means "no formula". `changed`: the field holds
   * something else than the source it opened with. Called once, unless
   * `cancel` is.
   */
  apply: (latex: string, changed: boolean) => void;
  /**
   * Nothing is to change. `dropped`: closed while locked with something
   * written in it, which is lost (to be said). Called once, unless `apply` is.
   */
  cancel: (dropped: boolean) => void;
  page: Document;
  /** Whose box it is (the editor's view): for `lockMathEditing` and `mathDraftChanged`. */
  owner?: object;
}

/** Builds the dialog controls before any listeners for its lifetime are installed. */
function mathDialog(page: Document, options: MathEditingOptions) {
  const box = page.createElement('div');
  box.setAttribute('class', 'doc-math-editing');
  box.setAttribute('role', 'dialog');
  box.setAttribute('aria-label', m.editor_math_edit());

  const field = page.createElement('textarea');
  field.setAttribute('aria-label', m.editor_math_source());
  field.setAttribute('maxlength', String(MAX_LATEX));
  field.setAttribute('rows', '1');
  field.setAttribute('spellcheck', 'false');
  field.setAttribute('autocapitalize', 'off');
  field.setAttribute('autocomplete', 'off');
  field.value = options.latex;

  const preview = page.createElement('div');
  preview.setAttribute('class', 'doc-math-preview doc-body');
  preview.setAttribute('aria-live', 'polite');

  const hint = page.createElement('p');
  hint.setAttribute('class', 'doc-math-hint');
  // Said when it changes: why the field no longer takes typing, and when it does again.
  hint.setAttribute('aria-live', 'polite');
  hint.append(page.createTextNode(m.editor_math_hint()));

  const buttons = page.createElement('div');
  buttons.setAttribute('class', 'doc-math-buttons');
  const button = (label: string, kind: string, run: () => void): HTMLButtonElement => {
    const element = page.createElement('button');
    element.setAttribute('type', 'button');
    element.dataset.action = kind;
    element.append(page.createTextNode(label));
    element.addEventListener('click', run);
    buttons.append(element);
    return element;
  };

  return { box, field, preview, hint, buttons, button };
}

/** Places the dialog near its anchor, keeping it inside the window. */
function placeMathDialog(box: HTMLElement, options: MathEditingOptions, view: Window | null): void {
  const rect = options.anchor();
  if (!rect || !view) return;
  const size = box.getBoundingClientRect();
  const left = Math.max(8, Math.min(rect.left, view.innerWidth - size.width - 8));
  const below = rect.bottom + 6;
  const fits = below + size.height <= view.innerHeight || rect.top - 6 - size.height < 0;
  box.style.left = `${left}px`;
  box.style.top = `${fits ? below : rect.top - 6 - size.height}px`;
}

/** Draws the field's source into the preview as it is typed: aside, and swapped in when ready. */
function previewer(
  page: Document,
  box: HTMLElement,
  field: HTMLTextAreaElement,
  preview: HTMLElement,
  closed: () => boolean,
): () => void {
  let sequence = 0;
  return () => {
    const latex = field.value.trim();
    sequence += 1;
    const mine = sequence;
    if (latex === '') {
      preview.replaceChildren();
      return;
    }
    const slot = page.createElement(MATH_SLOTS.inlineMath.tag);
    slot.setAttribute('class', MATH_CLASS);
    slot.setAttribute('data-type', MATH_SLOTS.inlineMath.type);
    slot.setAttribute(MATH_SOURCE_ATTRIBUTE, latex);
    slot.append(page.createTextNode(latex));
    const holder = page.createElement('div');
    holder.hidden = true;
    holder.append(slot);
    box.append(holder);
    // So that the preview does not blink.
    void fillMath(slot)
      .catch(() => undefined)
      .then(() => {
        holder.remove();
        if (mine === sequence && !closed()) preview.replaceChildren(slot);
      });
  };
}

/** The keys of the field: Escape cancels, Enter keeps. */
function fieldKeys(apply: () => void, cancel: () => void): (event: KeyboardEvent) => void {
  return (event) => {
    // While an input method is composing, its keys are not commands.
    if (event.isComposing) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      cancel();
    } else if (event.key === 'Enter') {
      event.preventDefault();
      apply();
    }
  };
}

let open:
  | {
      leave: () => void;
      cancel: () => void;
      owner?: object;
      changed: () => boolean;
      lock: (locked: boolean) => void;
    }
  | undefined;

/** Opens the box. Only one is open at a time; opening another applies the first. */
export function editMath(options: MathEditingOptions): void {
  open?.leave();
  const { page } = options;
  const view = page.defaultView;
  const { box, field, preview, hint, buttons, button } = mathDialog(page, options);
  let done = false;
  /** The editor is read-only for now: nothing is typed in, nothing kept (`lockMathEditing`). */
  let locked = false;
  const changed = (): boolean =>
    field.value.trim().slice(0, MAX_LATEX) !== options.latex.trim().slice(0, MAX_LATEX);

  const redraw = previewer(page, box, field, preview, () => done);

  function place(): void {
    placeMathDialog(box, options, view);
  }

  function close(): void {
    if (done) return;
    done = true;
    if (open?.cancel === cancel) open = undefined;
    view?.removeEventListener('resize', place);
    page.removeEventListener('pointerdown', outside, true);
    page.removeEventListener('click', outside, true);
    box.remove();
  }
  function apply(): void {
    if (done || locked) return;
    const latex = field.value.trim().slice(0, MAX_LATEX);
    const written = changed();
    close();
    options.apply(latex, written);
  }
  function cancel(): void {
    if (done) return;
    const dropped = locked && changed();
    close();
    options.cancel(dropped);
  }
  /** Leaving keeps what was written, like leaving a field; locked, it closes keeping nothing. */
  function leave(): void {
    if (locked) cancel();
    else apply();
  }
  const elsewhere = (event: Event): boolean =>
    !(event.target instanceof Node && box.contains(event.target));
  // Locked, only a press (a click), not a touch that only scrolls the page, closes it.
  const outside = (event: Event): void => {
    if (elsewhere(event) && (event.type === 'click') === locked) leave();
  };
  box.addEventListener('focusout', (event) => {
    const next = event.relatedTarget;
    if (next instanceof Node && !box.contains(next)) leave();
  });
  field.addEventListener('input', redraw);
  field.addEventListener('keydown', fieldKeys(apply, cancel));
  button(m.ui_done(), 'apply', apply);
  button(m.editor_math_remove(), 'remove', () => {
    field.value = '';
    apply();
  });

  box.append(field, preview, hint, buttons);
  page.body.append(box);
  open = { leave, cancel, owner: options.owner, changed, lock: lockBox };
  function lockBox(lock: boolean): void {
    if (lock === locked) return;
    locked = lock;
    field.readOnly = lock;
    for (const each of buttons.querySelectorAll('button')) each.disabled = lock;
    hint.replaceChildren(page.createTextNode(lock ? m.editor_math_locked() : m.editor_math_hint()));
  }
  place();
  redraw();
  view?.addEventListener('resize', place);
  page.addEventListener('pointerdown', outside, true);
  page.addEventListener('click', outside, true);
  field.focus();
  field.select();
}

/**
 * Locks the box of `owner`, if it is open, while its editor is read-only:
 * nothing more is typed into it and nothing is kept from it, and it says
 * why; unlocked when the editor may be written again. Escape still closes it.
 */
export function lockMathEditing(owner: object, locked: boolean): void {
  if (open?.owner === owner) open.lock(locked);
}

/**
 * Closes the box of `owner`, if it is open, keeping nothing: for when the
 * editor behind it goes away or is handed another document, and a late
 * change would land on something it was not written for.
 */
export function cancelMathEditingOf(owner: object): void {
  if (open?.owner === owner) open.cancel();
}

/** Whether the box of `owner` is open with a source other than the one it opened with. */
export function mathDraftChanged(owner: object): boolean {
  return open?.owner === owner && open.changed();
}
