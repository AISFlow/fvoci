/** The search box of a view: which cards mention the text. */
import { cardNumberOf } from './card-numbers.ts';
import { foldCase } from './text.ts';
import { cardProperties, plainPropertyValue, type Card, type PropertyTemplate } from './wire.ts';

/**
 * The cards whose title or one of whose property values contains the text,
 * without regard to case (see `foldCase`). A select or multi-select value is
 * searched by the labels of its options. Date and relation values are not searched.
 * A number (`12` or `#12`) finds the card with that number too.
 */
export function searchFilterCards(
  cards: readonly Card[],
  templates: readonly PropertyTemplate[],
  searchText: string,
): Card[] {
  const needle = foldCase(searchText);
  if (!needle) return cards.slice();
  const numbered = cardNumberOf(searchText);
  const mentions = (text: string | undefined): boolean =>
    text !== undefined && foldCase(text).includes(needle);

  return cards.filter((card) => {
    if (mentions(card.title)) return true;
    if (numbered !== undefined && card.number === numbered) return true;

    for (const [propertyId, value] of Object.entries(cardProperties(card))) {
      const template = templates.find((candidate) => candidate.id === propertyId);
      // A stored value can be null; the type only says what the editors write.
      // Dates have no text to search, and a relation only names cards by identifier.
      if (!template || !(value as typeof value | null)) continue;
      if (template.type === 'date' || template.type === 'relation') continue;
      const plain = plainPropertyValue(value) ?? [];
      const items = Array.isArray(plain) ? plain : [plain];
      const isOption = template.type === 'select' || template.type === 'multiSelect';
      const texts = isOption
        ? items.map((id) => template.options.find((option) => option.id === id)?.label)
        : items;
      if (texts.some(mentions)) return true;
    }
    return false;
  });
}
