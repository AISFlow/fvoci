/**
 * The shortest way from one list to another (Myers's O(ND) difference): what
 * of the first stays, what goes, and what comes. The lists are of keys, the
 * same key being the same item.
 *
 * What both lists start and end with is taken off first, which costs one
 * look at each item: an edit of a long list leaves a short middle. Only that
 * middle is searched, and the search spends a `DiffBudget`. When the budget
 * runs out, or the middle differs in more than `MAX_EDITS` places, the middle
 * is said to go and to come back whole (`coarse`): the answer is still true,
 * only less fine, and the page is never held for long.
 */

/** What becomes of a stretch of items: kept, gone from the first list, or come in the second. */
export type DiffStep = 'same' | 'removed' | 'added';

/** A stretch of items: `from` in the first list and `to` in the second, `length` items long. */
export interface DiffRun {
  step: DiffStep;
  /** Where it starts in the first list (for `added`, where it comes in). */
  from: number;
  /** Where it starts in the second list (for `removed`, where it was). */
  to: number;
  length: number;
}

/**
 * How much searching is left, in steps of the search: shared by every list
 * that one comparison of two bodies compares, so that a body of many changed
 * blocks costs no more than one long one.
 */
export class DiffBudget {
  left: number;
  /** Whether a search ran out of it and answered coarsely. */
  spent = false;

  constructor(steps: number) {
    this.left = steps;
  }

  /** Takes `steps` from what is left; false, and nothing taken, when there is not as much. */
  take(steps: number): boolean {
    if (steps > this.left) {
      this.spent = true;
      return false;
    }
    this.left -= steps;
    return true;
  }
}

/**
 * Past this many places that differ, a middle is not searched further. The
 * search keeps a row of the size of the places so far for every place, so
 * that this also bounds what it holds (about MAX_EDITS² numbers).
 */
export const MAX_EDITS = 2000;

/** Adds a run, joined to the one before when it is of the same step and follows on from it. */
function push(runs: DiffRun[], step: DiffStep, from: number, to: number, length: number): void {
  if (length <= 0) return;
  const last = runs.at(-1);
  if (
    last?.step === step &&
    last.from + (step === 'added' ? 0 : last.length) === from &&
    last.to + (step === 'removed' ? 0 : last.length) === to
  ) {
    last.length += length;
    return;
  }
  runs.push({ step, from, to, length });
}

/**
 * The places of the middle from `aStart` and `bStart`, `n` and `m` long,
 * found step by step (Myers): the rows of the search, one for each number of
 * places, or undefined when it ran out of the budget.
 */
function search(
  a: readonly string[],
  b: readonly string[],
  aStart: number,
  bStart: number,
  n: number,
  m: number,
  budget: DiffBudget,
): Int32Array[] | undefined {
  const max = n + m;
  const offset = max + 1;
  const reach = new Int32Array(2 * max + 3);
  const rows: Int32Array[] = [];
  for (let edits = 0; edits <= max; edits += 1) {
    if (edits > MAX_EDITS || !budget.take(2 * edits + 1)) return undefined;
    // What was reached with one place fewer, from diagonal -edits to edits.
    rows.push(reach.slice(offset - edits, offset + edits + 1));
    for (let k = -edits; k <= edits; k += 2) {
      const down =
        k === -edits ||
        (k !== edits && (reach[offset + k - 1] ?? 0) < (reach[offset + k + 1] ?? 0));
      let x = down ? (reach[offset + k + 1] ?? 0) : (reach[offset + k - 1] ?? 0) + 1;
      let y = x - k;
      const before = x;
      while (x < n && y < m && a[aStart + x] === b[bStart + y]) {
        x += 1;
        y += 1;
      }
      if (x - before > 0 && !budget.take(x - before)) return undefined;
      reach[offset + k] = x;
      if (x >= n && y >= m) return rows;
    }
  }
  return rows;
}

/** Walks the rows of a search back from the end of the middle: its runs, first to last. */
function walkBack(
  rows: Int32Array[],
  aStart: number,
  bStart: number,
  n: number,
  m: number,
): DiffRun[] {
  const steps: { step: DiffStep; x: number; y: number }[] = [];
  let x = n;
  let y = m;
  for (let edits = rows.length - 1; edits > 0; edits -= 1) {
    const row = rows[edits];
    if (!row) break;
    const k = x - y;
    const at = (diagonal: number): number => row[diagonal + edits] ?? 0;
    const down = k === -edits || (k !== edits && at(k - 1) < at(k + 1));
    const previousK = down ? k + 1 : k - 1;
    const previousX = at(previousK);
    const previousY = previousX - previousK;
    while (x > previousX && y > previousY) {
      x -= 1;
      y -= 1;
      steps.push({ step: 'same', x, y });
    }
    if (down) steps.push({ step: 'added', x: previousX, y: previousY });
    else steps.push({ step: 'removed', x: previousX, y: previousY });
    x = previousX;
    y = previousY;
  }
  while (x > 0 && y > 0) {
    x -= 1;
    y -= 1;
    steps.push({ step: 'same', x, y });
  }
  const runs: DiffRun[] = [];
  for (const one of steps.reverse()) push(runs, one.step, aStart + one.x, bStart + one.y, 1);
  return runs;
}

/**
 * Within each stretch that is not kept, what goes is put before what comes:
 * the search may weave them, which says the same but reads worse.
 */
function goneBeforeCome(runs: DiffRun[]): DiffRun[] {
  const out: DiffRun[] = [];
  let removed: DiffRun | undefined;
  let added: DiffRun | undefined;
  const flush = (): void => {
    if (removed) push(out, 'removed', removed.from, removed.to, removed.length);
    if (added) push(out, 'added', added.from, added.to, added.length);
    removed = undefined;
    added = undefined;
  };
  for (const run of runs) {
    if (run.step === 'same') {
      flush();
      push(out, 'same', run.from, run.to, run.length);
    } else if (run.step === 'removed') {
      if (removed) removed.length += run.length;
      else removed = { ...run, to: added?.to ?? run.to };
    } else if (added) added.length += run.length;
    else added = { ...run, from: removed ? removed.from + removed.length : run.from };
  }
  flush();
  return out;
}

/**
 * How `a` becomes `b`, as runs in order: every item of `a` is in one run
 * that is `same` or `removed`, every item of `b` in one that is `same` or
 * `added`, and within a stretch that differs what goes comes first.
 */
export function diffSequence(
  a: readonly string[],
  b: readonly string[],
  budget: DiffBudget,
): DiffRun[] {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let aEnd = a.length;
  let bEnd = b.length;
  while (aEnd > start && bEnd > start && a[aEnd - 1] === b[bEnd - 1]) {
    aEnd -= 1;
    bEnd -= 1;
  }
  const runs: DiffRun[] = [];
  push(runs, 'same', 0, 0, start);
  const n = aEnd - start;
  const m = bEnd - start;
  const rows = n > 0 && m > 0 ? search(a, b, start, start, n, m, budget) : undefined;
  if (rows) {
    for (const run of walkBack(rows, start, start, n, m))
      push(runs, run.step, run.from, run.to, run.length);
  } else {
    push(runs, 'removed', start, start, n);
    push(runs, 'added', aEnd, start, m);
  }
  push(runs, 'same', aEnd, bEnd, a.length - aEnd);
  return goneBeforeCome(runs);
}
