/**
 * Which cell of the table is being edited, and noticing when that is over.
 *
 * A table of hundreds of rows draws every value as plain text and chips.
 * Only the one cell the reader pressed gets the editor of its property
 * (`PropertyValue` in its editing mode, opened at once), and when the reader
 * is done the cell goes back to showing.
 *
 * The property components do not report that they are done, so it is read
 * off the cell: a field is done when the focus leaves the cell, and a picker
 * (a list of options, a calendar) is done when it closes, which its trigger
 * says with `aria-expanded`.
 */
import { tick } from 'svelte';

export interface EditedCell {
  rowId: string;
  columnId: string;
}

export class CellEditing {
  /** The cell that shows its editor, if one does. */
  current = $state.raw<EditedCell | undefined>(undefined);
  readonly #find: (at: EditedCell) => HTMLElement | undefined;

  /** `find` gives the element of a cell, or undefined when the table does not show it. */
  constructor(find: (at: EditedCell) => HTMLElement | undefined) {
    this.#find = find;
  }

  start(rowId: string, columnId: string): void {
    if (this.current?.rowId === rowId && this.current.columnId === columnId) return;
    this.current = { rowId, columnId };
  }

  /**
   * Back to showing. With `refocus` the focus goes to the cell itself, unless
   * the reader has already put it somewhere else: the keyboard continues from
   * where it was.
   */
  stop(refocus = false): void {
    const at = this.current;
    if (!at) return;
    this.current = undefined;
    if (!refocus) return;
    void tick().then(() => {
      const cell = this.#find(at);
      const active = document.activeElement;
      if (cell && (!active || active === document.body || cell.contains(active))) {
        cell.focus({ preventScroll: true });
      }
    });
  }

  /**
   * For an effect: follows the cell being edited until the editing is over.
   * Returns what stops following.
   */
  readonly watch = (): (() => void) | undefined => {
    const at = this.current;
    if (!at) return undefined;
    const cell = this.#find(at);
    if (!cell) {
      // The row is not shown (a filter or the search hides it, or it is gone).
      this.current = undefined;
      return undefined;
    }

    const onFocusOut = (event: FocusEvent): void => {
      const next = event.relatedTarget;
      if (next instanceof Node && cell.contains(next)) return;
      // The focus went into the picker this cell opened.
      if (cell.querySelector('[aria-expanded="true"]')) return;
      // A text that is not acceptable stays in its field, marked, until it is fixed or given up.
      if (cell.querySelector('[aria-invalid="true"]')) return;
      this.stop(next === null);
    };
    const observer = new MutationObserver((records) => {
      const closed = records.some(
        (record) =>
          record.oldValue === 'true' &&
          record.target instanceof Element &&
          record.target.getAttribute('aria-expanded') !== 'true',
      );
      if (closed) this.stop(true);
    });
    cell.addEventListener('focusout', onFocusOut);
    observer.observe(cell, {
      subtree: true,
      attributes: true,
      attributeFilter: ['aria-expanded'],
      attributeOldValue: true,
    });
    return () => {
      cell.removeEventListener('focusout', onFocusOut);
      observer.disconnect();
    };
  };
}
