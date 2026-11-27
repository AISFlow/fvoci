/**
 * The keyboard's place in the table, kept in the document itself.
 *
 * The table is one stop of the Tab key: exactly one cell can be tabbed to,
 * the one the reader was last in, and the arrow keys move from cell to cell.
 * With hundreds of rows that must not cost a re-render of every row, so the
 * `tabindex` of the cells is not drawn from state. Every cell is drawn with
 * `tabindex="-1"`, and this class turns the one cell that has the focus into
 * the stop (and back) as the focus moves.
 *
 * What the markup provides:
 *
 *   data-nav-row           a row the arrow keys stop in
 *   data-row="<id>"        on that row: what it is about (a card, a group)
 *   data-cell              a cell; `data-col="<id>"` names its column
 *   data-inner             a control inside a cell that the Tab key reaches
 *                          only while its cell is the stop
 *   data-pinned            a cell that stays at the left edge
 *   data-sticky="top|bottom"  the rows that stay at the top and at the bottom
 */

const ROW = '[data-nav-row]';
const CELL = '[data-cell]';

export type MoveKey = 'ArrowUp' | 'ArrowDown' | 'ArrowLeft' | 'ArrowRight' | 'Home' | 'End';

export function isMoveKey(key: string): key is MoveKey {
  return ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(key);
}

/** Where a key leads in a list of `length` things from `index`: never off either end. */
export function stepped(length: number, index: number, key: 'previous' | 'next'): number {
  const to = key === 'next' ? index + 1 : index - 1;
  return Math.max(0, Math.min(length - 1, to));
}

export class GridFocus {
  #root: HTMLElement | undefined;
  /** The cell that is the Tab stop. */
  #stop: HTMLElement | undefined;
  /** The column the reader moves up and down in, kept across rows that have one cell only. */
  #column: string | undefined;

  /** For `{@attach}` on the element with `role="grid"`. */
  readonly attach = (root: HTMLElement): (() => void) => {
    this.#root = root;
    root.addEventListener('focusin', this.#onFocusIn);
    this.ensure();
    return () => {
      root.removeEventListener('focusin', this.#onFocusIn);
      this.#root = undefined;
      this.#stop = undefined;
    };
  };

  readonly #onFocusIn = (event: FocusEvent): void => {
    const cell = this.cellOf(event.target);
    if (!cell) return;
    this.#take(cell);
    const column = cell.dataset.col;
    if (event.target === cell && column !== undefined && this.#cellsOf(cell).length > 1) {
      this.#column = column;
    }
  };

  /** Makes `cell` the Tab stop, and its inner controls reachable. */
  #take(cell: HTMLElement): void {
    const before = this.#stop;
    if (before && before !== cell) {
      before.tabIndex = -1;
      for (const inner of before.querySelectorAll<HTMLElement>('[data-inner]')) inner.tabIndex = -1;
    }
    cell.tabIndex = 0;
    for (const inner of cell.querySelectorAll<HTMLElement>('[data-inner]')) inner.tabIndex = 0;
    this.#stop = cell;
  }

  /**
   * After the rows or columns were drawn anew: there is still exactly one
   * stop. The arguments are what they were drawn from; they are not used,
   * but an effect that passes them runs again when they change.
   */
  ensure(..._drawnFrom: unknown[]): void {
    const root = this.#root;
    if (!root) return;
    if (this.#stop && root.contains(this.#stop)) {
      // Its inner controls may have been drawn again since.
      this.#take(this.#stop);
      return;
    }
    this.#stop = undefined;
    const first = root.querySelector<HTMLElement>(CELL);
    if (first) this.#take(first);
  }

  /** The cell an element is in, or is. */
  cellOf(target: EventTarget | null): HTMLElement | undefined {
    if (!(target instanceof Element)) return undefined;
    const cell = target.closest<HTMLElement>(CELL);
    return cell && this.#root?.contains(cell) ? cell : undefined;
  }

  /** The cell of a row (`data-row`) and column (`data-col`). */
  find(rowId: string, columnId: string): HTMLElement | undefined {
    const row = `${ROW}[data-row="${CSS.escape(rowId)}"]`;
    const cell = `${CELL}[data-col="${CSS.escape(columnId)}"]`;
    return this.#root?.querySelector<HTMLElement>(`${row} ${cell}`) ?? undefined;
  }

  #cellsOf(cell: HTMLElement): HTMLElement[] {
    const row = cell.closest(ROW);
    return row ? [...row.querySelectorAll<HTMLElement>(CELL)] : [cell];
  }

  /** The cell a key leads to from `cell`. `whole`: Home and End go to the first and last row. */
  next(cell: HTMLElement, key: MoveKey, whole = false): HTMLElement | undefined {
    const root = this.#root;
    if (!root) return undefined;
    const cells = this.#cellsOf(cell);
    const at = cells.indexOf(cell);

    if (key === 'ArrowLeft' || key === 'ArrowRight') {
      const forward = (key === 'ArrowRight') !== (getComputedStyle(root).direction === 'rtl');
      return cells[stepped(cells.length, at, forward ? 'next' : 'previous')];
    }
    if ((key === 'Home' || key === 'End') && !whole) {
      return key === 'Home' ? cells[0] : cells.at(-1);
    }

    const rows = [...root.querySelectorAll<HTMLElement>(ROW)];
    const row = cell.closest<HTMLElement>(ROW);
    const rowAt = row ? rows.indexOf(row) : -1;
    let target: HTMLElement | undefined;
    if (key === 'Home') target = rows[0];
    else if (key === 'End') target = rows.at(-1);
    else target = rows[stepped(rows.length, rowAt, key === 'ArrowDown' ? 'next' : 'previous')];
    if (!target || target === row) return key === 'Home' || key === 'End' ? cell : undefined;

    const wanted = this.#column ?? cell.dataset.col;
    const inRow = [...target.querySelectorAll<HTMLElement>(CELL)];
    if (key === 'Home') return inRow[0];
    if (key === 'End') return inRow.at(-1);
    return inRow.find((candidate) => candidate.dataset.col === wanted) ?? inRow[0];
  }

  /** Puts the focus on a cell and scrolls it out from under the pinned row and column. */
  focus(cell: HTMLElement): void {
    cell.focus({ preventScroll: true });
    this.reveal(cell);
  }

  /**
   * Scrolls so that a cell is in view. The browser's own scrolling does not
   * know that the heading row, the summary row and the title column cover
   * part of the box.
   */
  reveal(cell: HTMLElement): void {
    const root = this.#root;
    if (!root) return;
    const box = root.getBoundingClientRect();
    const rect = cell.getBoundingClientRect();
    const sizeOf = (selector: string): DOMRect | undefined =>
      root.querySelector(selector)?.getBoundingClientRect();

    if (!cell.closest('[data-sticky]')) {
      const top = box.top + (sizeOf('[data-sticky="top"]')?.height ?? 0);
      const bottom = box.top + root.clientHeight - (sizeOf('[data-sticky="bottom"]')?.height ?? 0);
      if (rect.top < top) root.scrollTop -= top - rect.top;
      else if (rect.bottom > bottom) root.scrollTop += rect.bottom - bottom;
    }
    if (!cell.hasAttribute('data-pinned')) {
      const pinned = sizeOf('[data-sticky="top"] [data-pinned]')?.width ?? 0;
      const rtl = getComputedStyle(root).direction === 'rtl';
      const left = rtl ? box.left : box.left + pinned;
      const right = box.left + root.clientWidth - (rtl ? pinned : 0);
      if (rect.left < left) root.scrollLeft -= left - rect.left;
      else if (rect.right > right)
        root.scrollLeft += Math.min(rect.right - right, rect.left - left);
    }
  }
}
