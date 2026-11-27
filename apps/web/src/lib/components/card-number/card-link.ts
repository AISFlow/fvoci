/**
 * The link of a card as it is copied: its whole address as text, which is
 * what a body of this app takes as a reference when it is pasted
 * (`editor/reference.ts`), and, for what takes a link with words (a mail, a
 * chat, a document), the same address under the card's number and title
 * (“#12 Fix the login”), marked as this app's (`COPIED_CARD_LINK`) so that
 * a body pastes the address and never those words. Where the browser writes
 * text only, the text alone.
 * A refusal of the browser (no permission, no secure page) is the caller's.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { COPIED_CARD_LINK, cardNumberText } from '$lib/logic/card-numbers.ts';

/** What a copied link says of the card: its number, where it has one, and its title. */
export function cardLinkText(card: { title: string; number?: number }): string {
  const title = card.title || m['KanbanCard.untitled']();
  return card.number === undefined ? title : `${cardNumberText(card.number)} ${title}`;
}

/** Puts the link to the card at `address` (a whole address, with its origin) on the clipboard. */
export async function copyCardLink(
  address: string,
  card: { title: string; number?: number },
): Promise<void> {
  const clipboard = navigator.clipboard;
  if (typeof ClipboardItem === 'undefined' || typeof clipboard.write !== 'function') {
    await clipboard.writeText(address);
    return;
  }
  const link = document.createElement('a');
  link.href = address;
  link.setAttribute(COPIED_CARD_LINK, '');
  link.textContent = cardLinkText(card);
  await clipboard.write([
    new ClipboardItem({
      'text/plain': new Blob([address], { type: 'text/plain' }),
      'text/html': new Blob([link.outerHTML], { type: 'text/html' }),
    }),
  ]);
}
