/**
 * The cells of one sheet as the worker reads them: columns of typed arrays
 * that grow as cells come, then put in order of row and column (a file may
 * write them in any order, and the same cell twice: the last one stands),
 * and cut to their length.
 */
import type { Sheet } from './xlsx-protocol.ts';

type Typed = Uint8Array | Uint16Array | Uint32Array | Float64Array;

/** A typed array that grows by doubling. */
class Growing<T extends Typed> {
  #array: T;
  #length = 0;
  readonly #make: (length: number) => T;

  constructor(make: (length: number) => T) {
    this.#make = make;
    this.#array = make(64);
  }

  get length(): number {
    return this.#length;
  }

  push(value: number): void {
    if (this.#length === this.#array.length) {
      const larger = this.#make(this.#array.length * 2);
      larger.set(this.#array);
      this.#array = larger;
    }
    this.#array[this.#length] = value;
    this.#length += 1;
  }

  at(index: number): number {
    return this.#array[index] ?? 0;
  }

  /** The values, in the order of `order` when given, in an array of their own length. */
  done(order?: Uint32Array): T {
    if (!order) return this.#array.slice(0, this.#length) as T;
    const out = this.#make(order.length);
    order.forEach((from, to) => {
      out[to] = this.#array[from] ?? 0;
    });
    return out;
  }
}

export class Cells {
  readonly #rows = new Growing((n) => new Uint32Array(n));
  readonly #columns = new Growing((n) => new Uint16Array(n));
  readonly #kinds = new Growing((n) => new Uint8Array(n));
  readonly #styles = new Growing((n) => new Uint32Array(n));
  readonly #values = new Growing((n) => new Float64Array(n));
  /** Whether every cell so far came after the one before. */
  #ordered = true;

  add(row: number, column: number, kind: number, style: number, value: number): void {
    const last = this.#rows.length - 1;
    if (last >= 0) {
      const before = this.#rows.at(last) * 16_384 + this.#columns.at(last);
      if (row * 16_384 + column <= before) this.#ordered = false;
    }
    this.#rows.push(row);
    this.#columns.push(column);
    this.#kinds.push(kind);
    this.#styles.push(style);
    this.#values.push(value);
  }

  /** The order that sorts the cells by place, keeping the last of each place; undefined when they are in it. */
  #order(): Uint32Array | undefined {
    if (this.#ordered) return undefined;
    const count = this.#rows.length;
    const keys = new Float64Array(count);
    for (let index = 0; index < count; index += 1) {
      keys[index] = this.#rows.at(index) * 16_384 + this.#columns.at(index);
    }
    // By place, and of one place the later first, so that the first of each run is kept.
    const sorted = Uint32Array.from({ length: count }, (_, index) => index).sort(
      (a, b) => (keys[a] ?? 0) - (keys[b] ?? 0) || b - a,
    );
    const kept: number[] = [];
    for (const index of sorted) {
      const previous = kept.at(-1);
      if (previous === undefined || keys[previous] !== keys[index]) kept.push(index);
    }
    return Uint32Array.from(kept);
  }

  done(): Pick<Sheet, 'cellRows' | 'cellColumns' | 'kinds' | 'styles' | 'values'> {
    const order = this.#order();
    return {
      cellRows: this.#rows.done(order),
      cellColumns: this.#columns.done(order),
      kinds: this.#kinds.done(order),
      styles: this.#styles.done(order),
      values: this.#values.done(order),
    };
  }
}
