/** Include or drop `item` in a checkbox-backed list without duplicates. */
export function toggleItem<T>(items: readonly T[], item: T, include: boolean): T[] {
  if (include) {
    return items.includes(item) ? [...items] : [...items, item];
  }
  return items.filter((value) => value !== item);
}
