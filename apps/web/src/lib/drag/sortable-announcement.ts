/** The words describing a drag's place, without reading the DOM. */
import { locateItem } from '$lib/logic/drop-target.ts';
import type { SortableActive, SortableOptions, SortableZonePlace } from './sortable-types.ts';

export function sortableWords(
  options: SortableOptions,
  kind: 'pickedUp' | 'moved' | 'dropped' | 'cancelled',
  active: SortableActive,
): string {
  const { messages } = options;
  const order = options.order();
  const item = options.itemLabel?.(active.itemId) ?? active.itemId;
  const items = active.itemIds.length;

  if (kind !== 'cancelled' && active.to?.kind === 'zone') {
    const place: SortableZonePlace = {
      item,
      items,
      zone: options.zoneLabel?.(active.to.zoneId) ?? active.to.zoneId,
    };
    const words =
      kind === 'dropped' ? messages.droppedInZone?.(place) : messages.movedToZone?.(place);
    return words ?? '';
  }

  // A cancelled item is back where it is in the data, among all the items there.
  const home = kind === 'cancelled' ? locateItem(order, active.itemId) : undefined;
  const at = home ?? active.gap ?? active.from;
  const list = order.find((candidate) => candidate.id === at.listId);
  const staying = list ? list.items.filter((id) => !active.itemIds.includes(id)).length : 0;
  return messages[kind]({
    item,
    items,
    list: options.listLabel?.(at.listId) ?? at.listId,
    position: at.index + 1,
    count: home ? (list?.items.length ?? 0) : staying + items,
  });
}
