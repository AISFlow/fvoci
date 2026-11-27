/**
 * Gives back the objects of last time for those that say the same now.
 *
 * A chart is worked out whole whenever a card changes, and most of it comes
 * out as it was. A row or an arrow that is the same object as before is not
 * drawn again, and with thousands of them that is most of the work of a
 * change.
 */

/** Whether two values say the same: equal, or plain objects whose members say the same. */
export function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  const first = a as Record<string, unknown>;
  const second = b as Record<string, unknown>;
  const keys = Object.keys(first);
  return (
    keys.length === Object.keys(second).length &&
    keys.every((key) => key in second && same(first[key], second[key]))
  );
}

export class Kept<T extends object> {
  #last = new Map<string, T>();

  /** `items`, each replaced by the object `keep` answered last time under its key where that says the same. */
  keep(items: readonly T[], keyOf: (item: T) => string): T[] {
    const next = new Map<string, T>();
    const kept = items.map((item) => {
      const key = keyOf(item);
      const before = this.#last.get(key);
      const one = before !== undefined && same(before, item) ? before : item;
      next.set(key, one);
      return one;
    });
    this.#last = next;
    return kept;
  }
}
