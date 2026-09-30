/** Optimistic holiday list used after add/remove before the list refetch. */
export function mergeHolidayItems(
  current: string[] | undefined,
  date: string,
  remove: boolean,
): string[] {
  return remove
    ? (current ?? []).filter((day) => day !== date)
    : [...new Set([...(current ?? []), date])].sort();
}
