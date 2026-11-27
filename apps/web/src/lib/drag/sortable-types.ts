/** Public data and options of a sortable, independent of DOM input handling. */
import type {
  Axis,
  Destination,
  DropTarget,
  ListOrder,
  Placement,
} from '$lib/logic/drop-target.ts';
import type { AutoScrollOptions } from './auto-scroll.ts';
import type { PointerDragOptions } from './pointer-drag.ts';

/**
 * A drop that changed something.
 *
 * `to` says what the drop means, and only a drop at a place has an index:
 * into a list that places items itself (`kind: 'list'`) and onto a zone
 * (`kind: 'zone'`) there is none, and where the items end up is for the
 * caller to decide.
 */
export interface SortableMove {
  /** The item that was grabbed. */
  itemId: string;
  /**
   * Everything that was dropped, in the order of the data: the grabbed item
   * alone, or the selection it was part of. `moveItems` takes it as it is.
   */
  itemIds: readonly string[];
  /** Where the grabbed item was, counted without the other dropped items. */
  from: DropTarget;
  to: Destination;
}

/** What is on its way. */
export interface SortableActive {
  /** The item that was grabbed. */
  itemId: string;
  /** Everything on its way, in the order of the data. One id unless a selection is dragged. */
  itemIds: readonly string[];
  mode: 'pointer' | 'keyboard';
  /** Who places the item in a list: the pointer or keys (`free`), or the list (`list`). */
  placement: Placement;
  /** Where the grabbed item was, counted without the others on their way. */
  from: DropTarget;
  /** What letting go now would do. Undefined while the pointer is outside the root: nothing, the drag is cancelled. */
  to: Destination | undefined;
  /**
   * Where the page shows the gap: at the place of the drop; where the list's
   * order would put the item when the list places items itself; where the
   * item came from while the pointer is outside. Undefined over a zone.
   */
  gap: DropTarget | undefined;
  /** The pointer is outside the root: letting go cancels. */
  outside: boolean;
  /** The size of the grabbed item when it was picked up, for the gap and the overlay. */
  width: number;
  height: number;
}

/** A place in words: for the announcements. `position` starts at 1. */
export interface SortablePlace {
  /** The name of the grabbed item. */
  item: string;
  /** How many items are on their way: 1 unless a selection is dragged. */
  items: number;
  list: string;
  position: number;
  count: number;
}

/** A zone in words. */
export interface SortableZonePlace {
  item: string;
  items: number;
  zone: string;
}

/**
 * The texts. The first four are read out to a screen reader; `sorted` and
 * `outside` are also shown on the page while they apply (`notice`).
 */
export interface SortableMessages {
  pickedUp(place: SortablePlace): string;
  moved(place: SortablePlace): string;
  dropped(place: SortablePlace): string;
  cancelled(place: SortablePlace): string;
  /**
   * That the view is sorted, so a dropped item goes where the order puts it
   * and only the list can be chosen. Needed where `placement` can be `list`.
   */
  sorted?(): string;
  /** That letting go now cancels: the pointer is outside the root. */
  outside?(): string;
  /** Needed where there are zones. */
  movedToZone?(place: SortableZonePlace): string;
  droppedInZone?(place: SortableZonePlace): string;
}

export interface SortableOptions {
  /** The order as data, as the page shows it, at the time of the call. */
  order: () => readonly ListOrder[];
  /** Called once per drop that changes something. */
  onDrop: (move: SortableMove) => void;
  messages: SortableMessages;
  /** Names for the announcements. Default: the ids. */
  itemLabel?: (itemId: string) => string;
  listLabel?: (listId: string) => string;
  zoneLabel?: (zoneId: string) => string;
  /** The direction in which each list stacks its items. Default `y`. */
  axis?: Axis;
  /**
   * Which of the marked elements under the root are this instance's: those
   * with `data-drag-scope="<scope>"`. Without a scope, those that have no
   * such attribute.
   */
  scope?: string;
  /**
   * Who places a dropped item in its list, asked when an item is picked up.
   * `list`: the view is sorted, so only the list can be chosen. Default `free`.
   */
  placement?: () => Placement;
  /**
   * With `placement` `list`: the index, counted without the items on their
   * way (`carried`), at which the order of that list would put the grabbed
   * item. The gap is drawn there. Without it the gap is at the end of the list.
   */
  sortedIndex?: (itemId: string, listId: string, carried: readonly string[]) => number | undefined;
  /**
   * The items selected now. When the grabbed item is one of them the drag
   * carries them all; otherwise it carries the grabbed item alone.
   */
  selection?: () => readonly string[];
  /** The ids of the zones the keyboard can reach, in order: they come after the last list. */
  zones?: () => readonly string[];
  pointer?: Partial<PointerDragOptions>;
  scroll?: Partial<AutoScrollOptions>;
}
