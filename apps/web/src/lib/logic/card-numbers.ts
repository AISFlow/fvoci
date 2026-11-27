/**
 * The numbers of a board's cards as a reader writes and finds them: `#12`
 * names the card numbered 12 on its board (the server numbers them from 1,
 * `Card.number`). A body never holds the number: what `#12` puts is a
 * reference to the card, by its identifier.
 */
import { foldCase } from './text.ts';
import type { Card } from './wire.ts';

/** The highest number a card can have (the contract's `int32`). */
const HIGHEST_NUMBER = 2_147_483_647;

/** How many cards `#` offers at most. */
export const NAMED_AT_MOST = 8;

/**
 * The attribute of the link that a card's copied link carries as HTML
 * (`components/card-number/card-link.ts`): the words under it are for other
 * apps, and a body of this app pastes the address under it instead.
 */
export const COPIED_CARD_LINK = 'data-card-link';

/**
 * The number a text names: `12` or `#12`, as `Card.number` writes it (no
 * leading zero, no sign, no space within); none for anything else.
 */
export function cardNumberOf(text: string): number | undefined {
  const match = /^#?([1-9]\d{0,9})$/.exec(text.trim());
  if (!match?.[1]) return undefined;
  const number = Number(match[1]);
  return number <= HIGHEST_NUMBER ? number : undefined;
}

/**
 * The positive card number named by a `#`-prefixed search query (`#12`, or
 * `#00012` normalized to `12`), matching the server's card-number search rule:
 * `#` followed solely by ASCII digits whose value is at least 1 and at most
 * `HIGHEST_NUMBER`. Returns `undefined` for bare digits, zero, mixed text, or
 * overflow.
 */
export function hashCardNumberOf(text: string): number | undefined {
  const match = /^#(\d+)$/.exec(text.trim());
  if (!match?.[1]) return undefined;
  const normalized = match[1].replace(/^0+/, '');
  if (normalized === '' || normalized.length > 10) return undefined;
  const number = Number(normalized);
  return number <= HIGHEST_NUMBER ? number : undefined;
}

/** How a card's number is shown: `#12`. */
export function cardNumberText(number: number): string {
  return `#${String(number)}`;
}

/**
 * The cards that what was typed after `#` names, `NAMED_AT_MOST` at most:
 * for digits, the card with that number first, then those whose number
 * begins with them, lowest first; then, as for any other text, the cards
 * whose title has every word of it, whatever the case, the one changed last
 * first. Spaces around what was typed do not count. Nothing for nothing
 * typed.
 */
export function cardsNamed(cards: readonly Card[], typed: string): Card[] {
  const words = foldCase(typed)
    .split(/\s+/u)
    .filter((word) => word !== '');
  if (words.length === 0) return [];
  const [first] = words;
  const digits =
    words.length === 1 && first !== undefined && /^\d+$/.test(first) ? first : undefined;
  const byNumber = digits
    ? cards
        .filter((card) => card.number !== undefined && String(card.number).startsWith(digits))
        .sort((a, b) => (a.number ?? 0) - (b.number ?? 0))
    : [];
  const taken = new Set(byNumber.map((card) => card.id));
  const byTitle = cards
    .filter((card) => !taken.has(card.id))
    .filter((card) => words.every((word) => foldCase(card.title).includes(word)))
    .sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
  return [...byNumber, ...byTitle].slice(0, NAMED_AT_MOST);
}
