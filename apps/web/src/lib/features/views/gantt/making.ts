/**
 * The header's "New" on a Gantt chart: a card of one day,
 * today, or the first day wholly in sight where today is not; with the
 * values the view's filter asks for (`newCardProperties`). It opens, as a
 * card made on a board does. One at a time: a second "New" while the first
 * card is being made makes nothing.
 */
import { newCardProperties } from '$lib/logic/calendar.ts';
import { addDays, type CalendarDate } from '$lib/logic/dates.ts';
import type { Axis } from '$lib/logic/gantt.ts';
import type { BoardView, PropertyTemplate } from '$lib/logic/wire.ts';
import { whileHere, type BoardStore } from '$lib/state/index.ts';

export interface MakingSource {
  board: () => BoardStore;
  view: () => BoardView;
  /** The date property the chart is placed by. */
  template: () => PropertyTemplate | undefined;
  axis: () => Axis;
  today: () => CalendarDate;
  /** The box that scrolls; undefined while it is not drawn. */
  scroller: () => HTMLElement | undefined;
  /** How wide the days in sight are, in pixels. */
  room: () => number;
  /** The size of 1rem in pixels. */
  rem: () => number;
  onOpenCard: (cardId: string) => void;
}

/** The first and the last day wholly in sight on `axis`, scrolled `left` pixels in. */
export function daysInSight(
  axis: Axis,
  left: number,
  room: number,
  rem: number,
): { first: CalendarDate; last: CalendarDate } {
  const day = axis.dayWidth * rem;
  const first = Math.min(axis.days - 1, Math.ceil(left / day));
  const last = Math.max(first, Math.floor((left + room) / day) - 1);
  return { first: addDays(axis.first, first), last: addDays(axis.first, last) };
}

/** Makes a card from the header's "New". */
export function ganttCardMaker(source: MakingSource): () => Promise<void> {
  let busy = false;
  return async () => {
    if (busy) return;
    busy = true;
    try {
      const board = source.board();
      const today = source.today();
      const sight = daysInSight(
        source.axis(),
        source.scroller()?.scrollLeft ?? 0,
        source.room(),
        source.rem(),
      );
      const day = today >= sight.first && today <= sight.last ? today : sight.first;
      const templates = board.board?.cardProperties ?? [];
      const filter = source.view().fields.filter;
      const properties = newCardProperties(filter, templates, source.template(), day);
      const card = await whileHere(board, () => board.createCard({ properties }));
      if (card) source.onOpenCard(card.id);
    } finally {
      busy = false;
    }
  };
}
