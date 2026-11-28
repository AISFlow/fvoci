/**
 * What the lines of a row's "more" menu do (`rowMenu`): open, duplicate,
 * copy the link, select and delete the card. After a duplicate or a delete the
 * keyboard goes on from a row that is there.
 */
import { tick } from 'svelte';

import { copyCardLink } from '$lib/components/card-number/card-link.ts';
import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import { TITLE_COLUMN_ID, type Card } from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

import type { Selection } from '../selection.svelte.ts';

import type { GridFocus } from './grid-focus.ts';
import { rowMenu } from './menus.ts';

export interface RowActionsHost {
  board: () => BoardStore;
  /** The cards the view shows, in its order. */
  cards: () => readonly Card[];
  cardHref: (cardId: string) => string;
  openCard: (cardId: string) => void;
  grid: GridFocus;
  selection: Selection;
  /** Tells a screen reader what happened without a sign of its own. */
  say: (text: string) => void;
}

async function focusTitle(host: RowActionsHost, cardId: string | undefined): Promise<void> {
  if (cardId === undefined) return;
  await tick();
  const cell = host.grid.find(cardId, TITLE_COLUMN_ID);
  if (cell) host.grid.focus(cell);
}

async function deleteCard(host: RowActionsHost, card: Card): Promise<void> {
  // The keyboard continues from the row that takes the place of the deleted one.
  const cards = host.cards();
  const at = cards.findIndex((other) => other.id === card.id);
  const neighbour = cards[at + 1] ?? cards[at - 1];
  await host.board().deleteCards([card.id]);
  await focusTitle(host, neighbour?.id);
}

async function duplicateCard(host: RowActionsHost, card: Card): Promise<void> {
  const copy = await host.board().duplicateCard(card.id);
  await focusTitle(host, copy?.id);
}

async function copyLink(host: RowActionsHost, card: Card): Promise<void> {
  // An anchor gives the whole address of a path: with the origin and the base path.
  const link = document.createElement('a');
  link.href = host.cardHref(card.id);
  try {
    await copyCardLink(link.href, card);
  } catch {
    // The browser refused (no permission, not a secure page): nothing was copied, and nothing is said.
    return;
  }
  host.say(m['CardActionsMenu.copiedLink']());
}

/** The lines of the "more" menu of the row of `card`. */
export function rowActionsOf(host: RowActionsHost, card: Card): MenuAction[] {
  return rowMenu({
    canEdit: host.board().canEdit,
    onOpen: () => {
      host.openCard(card.id);
    },
    onDuplicate: () => void duplicateCard(host, card),
    onCopyLink: () => void copyLink(host, card),
    onDelete: () => void deleteCard(host, card),
    selected: host.selection.has(card.id),
    onSelect: () => {
      host.selection.begin(card.id);
    },
  });
}
