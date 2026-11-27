/** What a screen reader is told while a row or a column of the table is moved. */
import type { SortableMessages, SortablePlace } from '$lib/drag/sortable.svelte.ts';
import { m } from '$lib/i18n/paraglide/messages.js';

/**
 * The words for moving a row. `groupName` names the group a list of rows
 * stands for, or returns undefined where the table has no groups.
 */
export function rowDragMessages(
  groupName: (listId: string) => string | undefined,
): SortableMessages {
  const within = (text: string, place: SortablePlace): string => {
    const group = groupName(place.list);
    return group === undefined ? text : `${text} ${m.table_group_named({ group })}`;
  };
  return {
    pickedUp: (place) => within(m.table_row_picked_up(place), place),
    moved: (place) => within(m.table_row_moved(place), place),
    dropped: (place) => within(m.table_row_dropped(place), place),
    cancelled: (place) => within(m.table_row_cancelled(place), place),
    outside: () => m.table_drag_outside(),
  };
}

export const COLUMN_DRAG_MESSAGES: SortableMessages = {
  pickedUp: (place) => m.table_column_picked_up(place),
  moved: (place) => m.table_column_moved(place),
  dropped: (place) => m.table_column_dropped(place),
  cancelled: (place) => m.table_column_cancelled(place),
  outside: () => m.table_drag_outside(),
};
