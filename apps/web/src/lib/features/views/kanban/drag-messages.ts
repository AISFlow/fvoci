/** The words of the kanban's two sortables, in the reader's language. */
import type {
  SortableMessages,
  SortablePlace,
  SortableZonePlace,
} from '$lib/drag/sortable.svelte.ts';
import { m } from '$lib/i18n/paraglide/messages.js';

/** The grabbed card by name, and how many travel with it. */
function itemOf(place: { item: string; items: number }): string {
  return place.items > 1
    ? m.kanban_drag_item_many({ item: place.item, more: place.items - 1 })
    : place.item;
}

const words = (place: SortablePlace): SortablePlace => ({ ...place, item: itemOf(place) });
const zoneWords = (place: SortableZonePlace): SortableZonePlace => ({
  ...place,
  item: itemOf(place),
});

/** Cards: within and between columns, and onto hidden columns. */
export const CARD_MESSAGES: SortableMessages = {
  pickedUp: (place) => m.kanban_drag_picked_up(words(place)),
  moved: (place) => m.kanban_drag_moved(words(place)),
  dropped: (place) => m.kanban_drag_dropped(words(place)),
  cancelled: (place) => m.kanban_drag_cancelled(words(place)),
  sorted: () => m.kanban_drag_sorted(),
  outside: () => m.kanban_drag_outside(),
  movedToZone: (place) => m.kanban_drag_moved_to_hidden(zoneWords(place)),
  droppedInZone: (place) => m.kanban_drag_dropped_in_hidden(zoneWords(place)),
};

/** Columns: along their row. */
export const COLUMN_MESSAGES: SortableMessages = {
  pickedUp: (place) => m.kanban_column_picked_up(place),
  moved: (place) => m.kanban_column_moved(place),
  dropped: (place) => m.kanban_column_dropped(place),
  cancelled: (place) => m.kanban_column_cancelled(place),
  outside: () => m.kanban_drag_outside(),
};
