/** What dragging a board in the sidebar says, to a screen reader and on the page. */
import type { SortableMessages } from '$lib/drag/sortable.svelte.ts';
import { m } from '$lib/i18n/paraglide/messages.js';

/**
 * A board is dragged alone (the sidebar has no selection), a list is an open
 * category, and a zone is a folded one.
 */
export function containerDragMessages(): SortableMessages {
  return {
    pickedUp: ({ item, position, count, list }) =>
      m.shell_drag_picked_up({ item, position, count, list }),
    moved: ({ item, position, count, list }) => m.shell_drag_moved({ item, position, count, list }),
    dropped: ({ item, position, count, list }) =>
      m.shell_drag_dropped({ item, position, count, list }),
    cancelled: ({ item, position, count, list }) =>
      m.shell_drag_cancelled({ item, position, count, list }),
    outside: () => m.shell_drag_outside(),
    movedToZone: ({ item, zone }) => m.shell_drag_over_category({ item, category: zone }),
    droppedInZone: ({ item, zone }) => m.shell_drag_dropped_in_category({ item, category: zone }),
  };
}
