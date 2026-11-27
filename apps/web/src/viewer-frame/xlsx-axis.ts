/**
 * The rows or the columns of a sheet as lengths along one axis: most are of
 * the default size, some of their own (a run of columns, a row), and the
 * hidden ones of none. Where an index starts and which index lies at a
 * position are found by searching the runs of their own size, never by
 * walking the indices: a sheet can reach a million rows with one cell.
 *
 * Sizes are in CSS pixels at 100%.
 */

/** A run of indices of one size of their own. */
interface Run {
  first: number;
  last: number;
  size: number;
  /** Where the run starts. */
  start: number;
}

export class Axis {
  /** How many indices there are. */
  readonly count: number;
  readonly #size: number;
  readonly #runs: Run[];
  /** Where the axis ends: its whole length. */
  readonly total: number;

  /**
   * `runs` are [first, last, size] triples, in any order; where they
   * overlap, the earlier wins. Those past `count` are dropped.
   */
  constructor(count: number, size: number, runs: readonly (readonly [number, number, number])[]) {
    this.count = Math.max(0, count);
    // A default of no size would put every index at one point.
    this.#size = Math.max(1, size);
    const sorted = [...runs]
      .map(([first, last, own]) => ({
        first,
        last: Math.min(last, this.count - 1),
        size: Math.max(0, own),
      }))
      .filter((run) => run.first <= run.last && run.first < this.count)
      .sort((a, b) => a.first - b.first);
    this.#runs = [];
    let end = -1;
    let shift = 0;
    for (const run of sorted) {
      const first = Math.max(run.first, end + 1);
      if (first > run.last) continue;
      const before = this.#runs.at(-1);
      // Next to a run of the same size it is that run, longer: a filtered sheet's thousands of
      // hidden rows are one run to step over, not thousands.
      if (before && before.last + 1 === first && before.size === run.size) before.last = run.last;
      else
        this.#runs.push({
          first,
          last: run.last,
          size: run.size,
          start: first * this.#size + shift,
        });
      shift += (run.last - first + 1) * (run.size - this.#size);
      end = run.last;
    }
    this.total = this.count * this.#size + shift;
  }

  /** The last run that starts at or before `index`, by its place in the list; -1 when none. */
  #runBefore(index: number): number {
    let low = 0;
    let high = this.#runs.length - 1;
    let found = -1;
    while (low <= high) {
      const middle = (low + high) >> 1;
      if ((this.#runs[middle]?.first ?? 0) <= index) {
        found = middle;
        low = middle + 1;
      } else high = middle - 1;
    }
    return found;
  }

  /** Where an index starts (an index at `count` is where the axis ends). */
  start(index: number): number {
    const at = this.#runBefore(index);
    const run = this.#runs[at];
    if (!run) return index * this.#size;
    if (index <= run.last) return run.start + (index - run.first) * run.size;
    const end = run.start + (run.last - run.first + 1) * run.size;
    return end + (index - run.last - 1) * this.#size;
  }

  /** The index after `index`, past any runs of hidden ones. */
  next(index: number): number {
    let next = index + 1;
    for (;;) {
      const run = this.#runs[this.#runBefore(next)];
      if (!run || next > run.last || run.size > 0) return next;
      next = run.last + 1;
    }
  }

  /** The indices of some size from `first` to `last`, in order, at most `most` of them. */
  sized(first: number, last: number, most: number): number[] {
    const found: number[] = [];
    for (let index = first; index <= last && found.length < most; index = this.next(index)) {
      if (this.size(index) > 0) found.push(index);
    }
    return found;
  }

  size(index: number): number {
    const run = this.#runs[this.#runBefore(index)];
    return run && index <= run.last ? run.size : this.#size;
  }

  /** The index at a position: the last one starting at or before it, among those of some size. */
  indexAt(position: number): number {
    if (this.count === 0) return 0;
    // The last run that starts at or before the position.
    let low = 0;
    let high = this.#runs.length - 1;
    let found = -1;
    while (low <= high) {
      const middle = (low + high) >> 1;
      if ((this.#runs[middle]?.start ?? 0) <= position) {
        found = middle;
        low = middle + 1;
      } else high = middle - 1;
    }
    const run = this.#runs[found];
    let index: number;
    if (!run) index = Math.floor(position / this.#size);
    else {
      const end = run.start + (run.last - run.first + 1) * run.size;
      if (position < end && run.size > 0)
        index = run.first + Math.floor((position - run.start) / run.size);
      else index = run.last + 1 + Math.floor(Math.max(0, position - end) / this.#size);
    }
    return Math.min(this.count - 1, Math.max(0, index));
  }
}
