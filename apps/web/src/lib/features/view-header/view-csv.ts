/**
 * The view shown as a CSV file the reader saves (logic/csv.ts): the cards it
 * shows now, filtered, sorted and searched, with the properties it shows.
 * People are named as the board names them, a related card by its title.
 *
 * Not offered on a public link: its views have their people taken out, and
 * a file of them would say something else than the board does.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { csvText, tableRows } from '$lib/logic/csv.ts';
import type { DisplayContext } from '$lib/logic/format.ts';
import { isGone, userDisplayName } from '$lib/logic/users.ts';
import { viewQuery, type BoardView, type Card } from '$lib/logic/wire.ts';
import { fileName, saveBlob } from '$lib/save.ts';
import type { BoardStore } from '$lib/state/index.ts';

import { viewTitle } from './texts.ts';

/** The name a person is shown by on the board, as a person property shows them. */
function personName(board: BoardStore): (userId: string) => string {
  const { workspace } = board;
  return (userId) => {
    const user = workspace.user(userId) ?? (userId === workspace.me?.id ? workspace.me : undefined);
    if (!user) return m['centerPanel.unknown-user']();
    // What is left of a deleted account has no name: the card keeps who it was set to.
    return isGone(user) ? m.people_deleted_user() : userDisplayName(user);
  };
}

/** The name of the file: the board's title and the view's, with nothing a system refuses. */
export function csvFileName(boardTitle: string, shownTitle: string): string {
  return fileName(`${boardTitle} - ${shownTitle}`, '.csv', 'view');
}

/** Saves the cards of the view, as it shows them, as a CSV file. */
export function saveViewCsv(board: BoardStore, view: BoardView, cards: readonly Card[]): void {
  const { workspace } = board;
  const context: DisplayContext = {
    locale: workspace.locale,
    timeZone: workspace.regional.timeZone,
    hourCycle: workspace.regional.hourCycle,
    cardTitle: (cardId) => board.cardsById.get(cardId)?.title,
    personName: personName(board),
  };
  const templates = board.board?.cardProperties ?? [];
  const rows = tableRows(
    cards,
    templates,
    viewQuery(view),
    m['default-properties.title'](),
    context,
  );
  const boardTitle = board.board?.title || m['Sidebar.untitled-board']();
  const blob = new Blob([csvText(rows)], { type: 'text/csv;charset=utf-8' });
  saveBlob(blob, csvFileName(boardTitle, viewTitle(view)));
}
