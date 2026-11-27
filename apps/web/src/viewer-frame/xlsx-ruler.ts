/**
 * The column letters above a sheet and the row numbers at its left, drawn
 * for the indices in view only, as the panes are (`xlsx-pane.ts`). They are
 * the sheet's frame, not its content: hidden from screen readers, which
 * have the cells' own indices.
 */
import type { Axis } from './xlsx-axis.ts';

/** The most labels one ruler draws: past them, the rest of it stays empty. */
export const LABEL_BUDGET = 1_000;
/** Labels nearer each other than this, in pixels, are left out. */
const LABEL_GAP = 4;

/** A column's letters: 0 is A, 25 Z, 26 AA. */
export function columnName(index: number): string {
  let name = '';
  for (let rest = index + 1; rest > 0; rest = Math.floor((rest - 1) / 26)) {
    name = String.fromCharCode(65 + ((rest - 1) % 26)) + name;
  }
  return name;
}

/** Where a ruler lies and what it shows: its box in the view, the indices it may show, and where along the sheet it starts. */
export interface Span {
  left: number;
  top: number;
  width: number;
  height: number;
  range: [number, number];
  /** Where its start lies along the sheet, in pixels at the zoom. */
  from: number;
}

export class Ruler {
  readonly element = document.createElement('div');
  readonly #axis: Axis;
  readonly #across: boolean;
  #drawn = new Map<number, HTMLElement>();

  /** `across` for the column letters, which run across; otherwise the row numbers. */
  constructor(axis: Axis, across: boolean) {
    this.#axis = axis;
    this.#across = across;
    this.element.className = across ? 'xlsx-ruler xlsx-ruler-across' : 'xlsx-ruler xlsx-ruler-down';
    this.element.setAttribute('aria-hidden', 'true');
  }

  clear(): void {
    this.element.replaceChildren();
    this.#drawn = new Map();
  }

  draw(span: Span, zoom: number): void {
    const box = this.element.style;
    box.left = `${span.left}px`;
    box.top = `${span.top}px`;
    box.width = `${Math.max(0, span.width)}px`;
    box.height = `${Math.max(0, span.height)}px`;
    const length = this.#across ? span.width : span.height;
    const touched = new Set<number>();
    if (length > 0 && span.range[0] < span.range[1]) {
      const axis = this.#axis;
      const first = Math.max(span.range[0], axis.indexAt(span.from / zoom));
      const last = Math.min(span.range[1] - 1, axis.indexAt((span.from + length) / zoom));
      for (
        let index = first;
        index <= last && touched.size < LABEL_BUDGET;
        index = axis.next(index)
      ) {
        const size = axis.size(index) * zoom;
        if (size < LABEL_GAP) continue;
        touched.add(index);
        const label = this.#label(index);
        const start = `${axis.start(index) * zoom - span.from}px`;
        label.style.setProperty(this.#across ? 'left' : 'top', start);
        label.style.setProperty(this.#across ? 'width' : 'height', `${size}px`);
      }
    }
    for (const [index, label] of this.#drawn) {
      if (!touched.has(index)) {
        label.remove();
        this.#drawn.delete(index);
      }
    }
  }

  #label(index: number): HTMLElement {
    let label = this.#drawn.get(index);
    if (!label) {
      label = document.createElement('div');
      label.className = 'xlsx-label';
      label.textContent = this.#across ? columnName(index) : String(index + 1);
      this.element.append(label);
      this.#drawn.set(index, label);
    }
    return label;
  }
}
