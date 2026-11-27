/**
 * Shared candidate sorting and search matching for card hierarchy pickers
 * (setting a parent card and attaching an existing card as a sub-card).
 */
import { foldCase } from '$lib/logic/text.ts';
import type { CardSummary } from '$lib/state/index.ts';

/** Maximum cards drawn at once inside a hierarchy picker list. */
export const HIERARCHY_PICKER_LIMIT = 50;

export interface PickerCardEntry {
  readonly card: CardSummary;
  readonly title: string;
  readonly foldedTitle: string;
  readonly numberText: string;
}

/** Sorts candidate cards alphabetically by title (then card ID) and precomputes search tokens. */
export function sortPickerCandidates(
  cards: readonly CardSummary[],
  locale: string,
  untitled: string,
): PickerCardEntry[] {
  const collator = new Intl.Collator(locale);
  return cards
    .map((card): PickerCardEntry => {
      const title = card.title || untitled;
      return {
        card,
        title,
        foldedTitle: foldCase(title),
        numberText: card.number === undefined ? '' : `#${card.number}`,
      };
    })
    .sort(
      (first, second) =>
        collator.compare(first.title, second.title) || (first.card.id < second.card.id ? -1 : 1),
    );
}

/** Filters pre-sorted candidates by title or `#number` substring. */
export function filterPickerCandidates(
  entries: readonly PickerCardEntry[],
  search: string,
): CardSummary[] {
  const query = foldCase(search.trim());
  if (query === '') return entries.map((entry) => entry.card);
  const plain = query.startsWith('#') ? query.slice(1) : query;
  const matched: CardSummary[] = [];
  for (const entry of entries) {
    const numberHit =
      entry.numberText !== '' &&
      (entry.numberText.includes(query) || (plain !== '' && entry.numberText.includes(plain)));
    if (entry.foldedTitle.includes(query) || numberHit) matched.push(entry.card);
  }
  return matched;
}

/** Texts passed to `PickList.textOf` so `PickList` keeps all pre-filtered candidates visible. */
export function pickerSearchTexts(card: CardSummary, untitled: string): string[] {
  const title = card.title || untitled;
  return card.number === undefined ? [title] : [title, `#${card.number}`, String(card.number)];
}
