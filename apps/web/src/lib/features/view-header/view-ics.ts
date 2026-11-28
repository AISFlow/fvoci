/**
 * The view shown as an iCalendar file (logic/ics.ts) the reader saves and
 * opens in a calendar: the cards it shows now, each on the days or at the
 * moments of the property the view puts them by (as a calendar view does,
 * `displayProperty`), a card without a value of it left out. A card is the
 * same event (`UID`) in every file of it, so that a calendar the file is
 * opened in again moves its events in place of adding them twice.
 *
 * Not offered on a public link, as the CSV of a view is not (view-csv.ts).
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { displayProperty } from '$lib/logic/calendar.ts';
import { icsCalendar, type IcsEvent } from '$lib/logic/ics.ts';
import type { DateValue } from '$lib/logic/dates.ts';
import { cardUpdatedAt } from '$lib/logic/card-time.ts';
import { parseDateValue, type BoardView, type Card } from '$lib/logic/wire.ts';
import { fileName, saveBlob } from '$lib/save.ts';
import type { BoardStore } from '$lib/state/index.ts';

import { viewTitle } from './texts.ts';

/** When a card is, by the property `propertyId` of the board; empty where it has no value of it. */
function whenOf(card: Card, propertyId: string, type: string): DateValue {
  if (type === 'createdTime') return { from: { instant: card.createdAt } };
  if (type === 'updatedTime') return { from: { instant: cardUpdatedAt(card) } };
  return parseDateValue(card.fields.properties[propertyId]);
}

/** Whether the view has a property to put its cards on days by. */
export function canSaveViewIcs(board: BoardStore, view: BoardView): boolean {
  return displayProperty(view.fields.dateProperty, board.board?.cardProperties ?? []) !== undefined;
}

/** Saves the cards of the view, as it shows them, as an iCalendar file. */
export function saveViewIcs(
  board: BoardStore,
  view: BoardView,
  cards: readonly Card[],
  cardHref: (cardId: string) => string,
): void {
  const template = displayProperty(view.fields.dateProperty, board.board?.cardProperties ?? []);
  if (!template) return;
  const events: IcsEvent[] = cards.map((card) => ({
    uid: `${card.id}@fvoci`,
    title: card.title || m['KanbanCard.untitled'](),
    when: whenOf(card, template.id, template.type),
    url: new URL(cardHref(card.id), location.href).href,
  }));
  const boardTitle = board.board?.title || m['Sidebar.untitled-board']();
  const name = `${boardTitle} - ${viewTitle(view)}`;
  const blob = new Blob([icsCalendar(name, events, Date.now())], {
    type: 'text/calendar;charset=utf-8',
  });
  saveBlob(blob, fileName(name, '.ics', 'calendar'));
}
