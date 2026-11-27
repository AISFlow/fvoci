/**
 * The comments that were being written when their card was closed. A draft
 * is kept by card for as long as the app is open, so that opening the card
 * again finds it; it is not stored anywhere and is gone with the tab.
 *
 * A plain map on purpose: nothing is drawn from it. The field reads its
 * draft once, when it is made, and writes back what is typed.
 */
const drafts = new Map<string, string>();

/** What was being written on a card; empty when nothing was. */
export function draftOf(cardId: string): string {
  return drafts.get(cardId) ?? '';
}

/** Remembers what is being written on a card; an empty text forgets it. */
export function keepDraft(cardId: string, text: string): void {
  if (text === '') drafts.delete(cardId);
  else drafts.set(cardId, text);
}
