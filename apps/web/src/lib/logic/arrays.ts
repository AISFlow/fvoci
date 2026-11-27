/** Small helpers for lists. */

/** Whether two lists have the same items in the same order, compared by identity. */
export function arraysEqual(
  first: readonly unknown[] | null | undefined,
  second: readonly unknown[] | null | undefined,
): boolean {
  if (first === second) return true;
  if (!first || !second) return false;
  if (first.length !== second.length) return false;
  return first.every((item, index) => item === second[index]);
}

/** Moves the item at `from` to position `to`, in place. */
export function arrayMove(list: unknown[], from: number, to: number): void {
  const moved = list.splice(from, 1);
  list.splice(to, 0, ...moved);
}
