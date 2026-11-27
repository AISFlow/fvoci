/**
 * The boards, wikis, diaries and courses the search offers to go to at once
 * ("바로 이동"): before anything is typed, the first of those the reader has,
 * in the order the sidebar has them; then those whose title holds what was
 * typed, whatever its case.
 */

/** How many are offered. */
export const JUMPS_SHOWN = 5;

interface Jumpable {
  isTemplate: boolean;
  deletedAt?: number | undefined;
}

export function searchJumps<T extends Jumpable>(
  containers: readonly T[],
  query: string,
  titleOf: (container: T) => string,
): T[] {
  const words = query.trim().toLocaleLowerCase();
  return containers
    .filter((container) => !container.isTemplate && container.deletedAt === undefined)
    .filter((container) => words === '' || titleOf(container).toLocaleLowerCase().includes(words))
    .slice(0, JUMPS_SHOWN);
}
