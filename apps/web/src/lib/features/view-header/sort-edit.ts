/**
 * Changes to the sort of a view: a list of properties, each ascending or
 * reversed. The cards are ordered by the first, cards equal in that by the
 * second, and so on (lib/logic/sort.ts). A function that has nothing to
 * change returns the list it was given.
 */
import { TITLE_COLUMN_ID, type PropertyTemplate, type SortOption } from '$lib/logic/wire.ts';

/**
 * What a sort can still be added on: the title and every property of the
 * board that the list does not have yet. A view sorts by a property once.
 */
export function sortCandidates(
  options: readonly SortOption[],
  templates: readonly PropertyTemplate[],
): string[] {
  const used = new Set(options.map((option) => option.propertyId));
  return [TITLE_COLUMN_ID, ...templates.map((template) => template.id)].filter(
    (id) => !used.has(id),
  );
}

/** Adds a sort at the end, ascending. */
export function addSort(options: readonly SortOption[], propertyId: string): readonly SortOption[] {
  if (options.some((option) => option.propertyId === propertyId)) return options;
  return [...options, { propertyId, reversed: false }];
}

export function reverseSort(options: readonly SortOption[], index: number): readonly SortOption[] {
  if (!options[index]) return options;
  return options.map((option, at) =>
    at === index ? { ...option, reversed: !option.reversed } : option,
  );
}

export function removeSort(options: readonly SortOption[], index: number): readonly SortOption[] {
  if (!options[index]) return options;
  return options.filter((_, at) => at !== index);
}

/** Moves the sort at `index` one place up (`-1`) or down (`1`). */
export function moveSort(
  options: readonly SortOption[],
  index: number,
  step: -1 | 1,
): readonly SortOption[] {
  const moved = options[index];
  const other = options[index + step];
  if (!moved || !other) return options;
  const next = [...options];
  next[index] = other;
  next[index + step] = moved;
  return next;
}
