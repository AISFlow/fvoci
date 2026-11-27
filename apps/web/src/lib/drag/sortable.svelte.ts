/**
 * Moving items within and between lists, with a pointer or the keyboard.
 *
 * The page marks its elements and draws; this class watches the input and
 * says what is going on.
 *
 *   data-drag-list="<id>"    the visible box of a list (what scrolls, if it
 *                            scrolls). The root itself may be one
 *   data-drag-item="<id>"    an item, inside its list
 *   data-drag-zone="<id>"    a target that is no list: a drop on it has no
 *                            place, only the zone
 *   data-drag-scroll="x|y"   a box that scrolls while dragging near its edge:
 *                            the root or anything between it and a list
 *   data-drag-handle         inside an item: only this part starts a drag
 *   data-drag-carried        the stand-in the page draws for the item while
 *                            the keyboard carries it; it receives the focus
 *   data-drag-scope="<name>" on a list, an item, a zone or a stand-in: which
 *                            instance it belongs to (see below)
 *
 * The item, or its handle when it has one, must be focusable: the keyboard
 * picks an item up from there.
 *
 * Nothing is moved in the data until the drop: `active` says which items are
 * on their way and what letting go would do, and the page draws that (the
 * items hidden where they were, a gap at `active.gap`, the grabbed item at
 * `overlay`). A dragged item must stay in the document, hidden, for as long
 * as it is dragged: a finger's touch events are sent to the element it landed
 * on.
 *
 * Two levels in one root. Several instances can share a root, each with its
 * own `scope`: the columns of a board sorted by one, the cards in the columns
 * by another. An instance reads only the lists, items and zones marked with
 * its scope (those without the attribute, when it has no scope), and an item
 * belongs to the nearest list of its scope around it. A press belongs to the
 * innermost item under it, of whatever scope, so one press never starts two
 * drags: pressing a card drags the card, pressing the heading of its column
 * (the column's handle) drags the column.
 */
import { tick } from 'svelte';

import {
  carriedItems,
  dropChanges,
  dropRegion,
  dropTarget,
  gapFor,
  intersectRect,
  keyboardDestination,
  locateItem,
  sameDestination,
  type Destination,
  type KeyboardStep,
  type Point,
  type Rect,
  windowChanged,
  type WindowSize,
} from '$lib/logic/drop-target.ts';

import { AutoScroller, type ScrollBox } from './auto-scroll.ts';
import { pointerDrag, type PointerDragHandle, type PointerDragSession } from './pointer-drag.ts';
import { DRAG_SETTINGS } from './settings.ts';

import { sortableWords } from './sortable-announcement.ts';
import { CARRIED, HANDLE, ITEM, LIST, placeAmong, ZONE, SortableDom } from './sortable-dom.ts';
import type { SortableActive, SortableOptions } from './sortable-types.ts';

export type {
  SortableActive,
  SortableMessages,
  SortableMove,
  SortableOptions,
  SortablePlace,
  SortableZonePlace,
} from './sortable-types.ts';

/** A space that is not shown and not trimmed away: it makes the same announcement a new one. */
const NO_BREAK_SPACE = String.fromCodePoint(0xa0);

function viewport(): Rect {
  return { left: 0, top: 0, width: window.innerWidth, height: window.innerHeight };
}

const windowSize = (): WindowSize => ({ width: window.innerWidth, height: window.innerHeight });

export class Sortable {
  #active = $state.raw<SortableActive | undefined>(undefined);
  #overlay = $state.raw<Point | undefined>(undefined);
  #announcement = $state('');

  readonly #options: SortableOptions;
  readonly #scroller: AutoScroller;
  readonly #page: SortableDom;
  #pointer: PointerDragHandle | undefined;
  /** The size of the window when nothing was being dragged: the size a drag began in. */
  #window: WindowSize = { width: 0, height: 0 };

  /** Where the pointer is, and where on the item it took hold. */
  #point: Point = { x: 0, y: 0 };
  #grab: Point = { x: 0, y: 0 };
  /** The pointer or something under it moved since the destination was last worked out. */
  #stale = false;
  #frame: number | undefined;
  #frameTime = 0;
  /** What had the focus when the item was picked up, and gets it back: the item or its handle. */
  #focused: 'item' | 'handle' | undefined;
  /** The last index the gap had in a list: where the keyboard comes back to from a zone. */
  #resumeIndex = 0;

  constructor(options: SortableOptions) {
    this.#options = options;
    this.#page = new SortableDom(options);
    this.#scroller = new AutoScroller(options.scroll);
  }

  /** What is on its way, or undefined when nothing is dragged. */
  get active(): SortableActive | undefined {
    return this.#active;
  }

  /** The top left corner of the item dragged by a pointer, in the coordinates of the viewport. */
  get overlay(): Point | undefined {
    return this.#overlay;
  }

  /** The latest announcement, to be put in a live region. */
  get announcement(): string {
    return this.#announcement;
  }

  /**
   * What the page shows in words while it applies, and nothing otherwise:
   * that letting go now cancels, or that the view is sorted and only the
   * list can be chosen.
   */
  get notice(): string {
    const active = this.#active;
    if (!active) return '';
    if (active.outside) return this.#options.messages.outside?.() ?? '';
    if (active.to?.kind === 'list') return this.#options.messages.sorted?.() ?? '';
    return '';
  }

  /** Attaches to the element that holds the lists: `{@attach sortable.attach}`. */
  readonly attach = (root: HTMLElement): (() => void) => {
    this.#page.root = root;
    const pointer = pointerDrag(root, this.#press, this.#options.pointer);
    this.#pointer = pointer;
    root.addEventListener('keydown', this.#onKeyDown);
    this.#window = windowSize();
    window.addEventListener('resize', this.#onResize);
    return () => {
      this.cancel();
      pointer.destroy();
      root.removeEventListener('keydown', this.#onKeyDown);
      window.removeEventListener('resize', this.#onResize);
      this.#pointer = undefined;
      this.#page.root = undefined;
    };
  };

  /**
   * A drag does not survive the window changing size under it (a device
   * unfolded or turned, a window put beside another): the lists are laid out
   * anew and nothing it measured holds. It is cancelled, with the usual
   * announcement. While nothing is dragged the size is only remembered.
   */
  readonly #onResize = (): void => {
    const size = windowSize();
    if (!this.#active) {
      this.#window = size;
      return;
    }
    if (windowChanged(this.#window, size, DRAG_SETTINGS.resizeTolerance)) {
      this.cancel();
      this.#window = size;
    }
  };

  /** Ends whatever is going on and puts everything back. */
  cancel(): void {
    this.#pointer?.cancel();
    this.#finish(false);
  }

  // ------------------------------------------------------------ the page

  // -------------------------------------------------------------- pick up

  /** The state of a drag that begins now with `item`, or undefined when the data no longer has it. */
  #pickUp(
    itemId: string,
    mode: SortableActive['mode'],
    item: HTMLElement,
  ): SortableActive | undefined {
    const order = this.#options.order();
    if (!locateItem(order, itemId)) return undefined;
    const itemIds = carriedItems(order, itemId, this.#options.selection?.());
    const from = locateItem(order, itemId, itemIds);
    if (!from) return undefined;
    const placement = this.#options.placement?.() ?? 'free';
    const to: Destination =
      placement === 'list' ? { kind: 'list', listId: from.listId } : { kind: 'index', ...from };
    const { width, height } = item.getBoundingClientRect();
    this.#resumeIndex = from.index;
    return { itemId, itemIds, mode, placement, from, to, gap: from, outside: false, width, height };
  }

  /** Points the drag at a new destination. Returns whether that changed anything. */
  #aim(to: Destination | undefined): boolean {
    const active = this.#active;
    if (!active || sameDestination(to, active.to)) return false;
    const gap = gapFor(to, {
      from: active.from,
      lists: this.#options.order(),
      carried: active.itemIds,
      sortedIndex: (listId) => this.#options.sortedIndex?.(active.itemId, listId, active.itemIds),
    });
    if (gap) this.#resumeIndex = gap.index;
    this.#active = { ...active, to, gap, outside: to === undefined };
    return true;
  }

  // ------------------------------------------------------------- pointer

  readonly #press = (event: PointerEvent): PointerDragSession | undefined => {
    if (this.#active) return undefined;
    const item = this.#page.itemAt(event.target);
    const itemId = item?.getAttribute(ITEM);
    if (!item || !itemId) return undefined;
    if (!locateItem(this.#options.order(), itemId)) return undefined;

    const pressed = item.getBoundingClientRect();
    const grab = { x: event.clientX - pressed.left, y: event.clientY - pressed.top };
    return {
      start: (point) => {
        const active = this.#pickUp(itemId, 'pointer', item);
        if (!active) return;
        this.#focused = this.#page.focusOn(item);
        this.#grab = grab;
        this.#point = point;
        this.#active = active;
        this.#overlay = { x: point.x - grab.x, y: point.y - grab.y };
        this.#page.root?.addEventListener('scroll', this.#onScroll, {
          capture: true,
          passive: true,
        });
        this.#stale = true;
        this.#frameTime = performance.now();
        this.#frame = requestAnimationFrame(this.#onFrame);
        if (active.placement === 'list') this.#say(this.#options.messages.sorted?.() ?? '');
      },
      move: (point) => {
        this.#point = point;
        if (!this.#active) return;
        this.#overlay = { x: point.x - this.#grab.x, y: point.y - this.#grab.y };
        this.#stale = true;
      },
      drop: (point) => {
        this.#point = point;
        this.#retarget();
        // Outside the root there is no destination, and this is a cancel.
        this.#finish(true);
      },
      cancel: () => {
        this.#finish(false);
      },
    };
  };

  readonly #onScroll = (): void => {
    this.#stale = true;
  };

  #scrollBoxes(): ScrollBox[] {
    return this.#page.scrollBoxes(this.#active?.to);
  }

  /**
   * Once per frame while a pointer drags: the destination follows the
   * pointer, the boxes scroll, and the destination follows what scrolled
   * under the pointer.
   */
  readonly #onFrame = (now: number): void => {
    if (this.#active?.mode !== 'pointer') return;
    const elapsed = Math.min(now - this.#frameTime, DRAG_SETTINGS.longestFrame);
    this.#frameTime = now;
    if (this.#stale) this.#retarget();
    if (this.#scroller.step(this.#point, this.#scrollBoxes(), elapsed)) this.#retarget();
    this.#frame = requestAnimationFrame(this.#onFrame);
  };

  /** Works out the destination from where the pointer and the items are now. */
  #retarget(): void {
    const root = this.#page.root;
    const active = this.#active;
    if (!root || !active) return;
    this.#stale = false;

    // The boxes of the lists and zones first, and then the items of the one
    // list the pointer picks: the cost of a frame does not grow with the
    // other lists.
    const bounds = intersectRect(root.getBoundingClientRect(), viewport());
    const axis = this.#options.axis;
    const lists: { element: HTMLElement; id: string; rect: DOMRect }[] = [];
    const zones: { id: string; rect: DOMRect }[] = [];
    for (const element of this.#page.all(LIST, ZONE)) {
      const rect = element.getBoundingClientRect();
      const listId = element.getAttribute(LIST);
      if (listId === null) zones.push({ id: element.getAttribute(ZONE) ?? '', rect });
      else lists.push({ element, id: listId, rect });
    }
    const region = dropRegion(this.#point, { bounds, zones, lists, axis });
    if (!region) return;

    if (region.kind === 'outside') {
      this.#aim(undefined);
    } else if (region.kind === 'zone') {
      this.#aim({ kind: 'zone', zoneId: region.zoneId });
    } else if (active.placement === 'list') {
      this.#aim({ kind: 'list', listId: region.list.id });
    } else {
      const { element, id, rect } = region.list;
      const nested = lists.some(
        (other) => other.element !== element && element.contains(other.element),
      );
      const staying = this.#page
        .itemsOf(element, nested)
        .filter((item) => !active.itemIds.includes(item.getAttribute(ITEM) ?? ''));
      const items = staying.map((item) => item.getBoundingClientRect());
      const reversed = axis === 'x' && this.#page.rtl();
      const place = dropTarget(this.#point, [{ id, rect, items }], { axis, bounds, reversed });
      if (place) this.#aim({ kind: 'index', ...place, index: placeAmong(staying, place.index) });
    }
  }

  // ------------------------------------------------------------ keyboard

  /** Space on a focused item, or on the focused handle of one, picks it up. */
  readonly #onKeyDown = (event: KeyboardEvent): void => {
    if (this.#active || event.key !== ' ' || event.repeat) return;
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const target = event.target;
    if (!(target instanceof HTMLElement)) return;
    const onHandle = !target.hasAttribute(ITEM) && target.hasAttribute(HANDLE);
    const item = onHandle ? target.closest<HTMLElement>(`[${ITEM}]`) : target;
    if (!item?.hasAttribute(ITEM) || !this.#page.isMine(item)) return;
    const active = this.#pickUp(item.getAttribute(ITEM) ?? '', 'keyboard', item);
    if (!active) return;

    event.preventDefault();
    this.#focused = onHandle ? 'handle' : 'item';
    this.#active = active;
    // On the window and capturing: the focus sits on the stand-in, which the
    // page may draw anywhere, and no other handler may act on these keys.
    window.addEventListener('keydown', this.#onCarryKey, true);
    window.addEventListener('pointerdown', this.#onCarryPointer, true);
    const sorted = active.placement === 'list' ? this.#options.messages.sorted?.() : undefined;
    this.#say([this.#words('pickedUp', active), sorted].filter(Boolean).join(' '));
    void this.#focusCarried();
  };

  readonly #onCarryKey = (event: KeyboardEvent): void => {
    const active = this.#active;
    if (active?.mode !== 'keyboard' || !active.to) return;
    const step = this.#stepFor(event.key);
    if (step) {
      const to = keyboardDestination(active.to, step, {
        lists: this.#options.order(),
        zones: this.#options.zones?.(),
        carried: active.itemIds,
        placement: active.placement,
        resumeIndex: this.#resumeIndex,
      });
      if (this.#aim(to)) {
        if (this.#active) this.#say(this.#words('moved', this.#active));
        void this.#focusCarried();
      } else if (active.to.kind === 'list' && step !== 'previousList' && step !== 'nextList') {
        // A key that would move within the list, where the list decides: say so again.
        this.#say(this.#options.messages.sorted?.() ?? '');
      }
    } else if (event.key === ' ' || event.key === 'Enter') {
      this.#finish(true);
    } else if (event.key === 'Escape') {
      this.#finish(false);
    } else if (event.key !== 'Tab') {
      return;
    }
    // Tab is swallowed too: the focus stays on the item until it is put down.
    event.preventDefault();
    event.stopPropagation();
  };

  /** Reaching for the pointer while carrying gives the carrying up. */
  readonly #onCarryPointer = (): void => {
    if (this.#active?.mode === 'keyboard') this.#finish(false);
  };

  /** The step a key stands for: along the axis within a list, across it between lists. */
  #stepFor(key: string): KeyboardStep | undefined {
    const vertical = (this.#options.axis ?? 'y') === 'y';
    switch (key) {
      case 'ArrowUp':
        return vertical ? 'previous' : 'previousList';
      case 'ArrowDown':
        return vertical ? 'next' : 'nextList';
      case 'ArrowLeft':
      case 'ArrowRight': {
        const forward = (key === 'ArrowRight') !== this.#page.rtl();
        if (vertical) return forward ? 'nextList' : 'previousList';
        return forward ? 'next' : 'previous';
      }
      case 'Home':
        return 'first';
      case 'End':
        return 'last';
      default:
        return undefined;
    }
  }

  /** Moves the focus to the stand-in once the page has drawn it at its new place. */
  async #focusCarried(): Promise<void> {
    await tick();
    const carried = this.#page.first(CARRIED);
    if (!carried) return;
    if (document.activeElement !== carried) carried.focus();
    carried.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }

  // ---------------------------------------------------------------- both

  /** Ends the drag. A commit without a destination (the pointer is outside the root) is a cancel. */
  #finish(commit: boolean): void {
    const active = this.#active;
    if (!active) return;

    if (this.#frame !== undefined) cancelAnimationFrame(this.#frame);
    this.#frame = undefined;
    this.#page.root?.removeEventListener('scroll', this.#onScroll, { capture: true });
    window.removeEventListener('keydown', this.#onCarryKey, true);
    window.removeEventListener('pointerdown', this.#onCarryPointer, true);

    const to = commit ? active.to : undefined;
    // A drop keeps the boxes where the drag scrolled them to; a cancel puts them back.
    if (to) this.#scroller.clear();
    else this.#scroller.restore();
    this.#say(this.#words(to ? 'dropped' : 'cancelled', active));
    this.#active = undefined;
    this.#overlay = undefined;
    if (to && dropChanges(this.#options.order(), active.itemIds, to)) {
      this.#options.onDrop({
        itemId: active.itemId,
        itemIds: active.itemIds,
        from: active.from,
        to,
      });
    }
    const focused = this.#focused;
    this.#focused = undefined;
    if (active.mode === 'keyboard') void this.#focusItem(active.itemId, focused, false);
    else if (focused) void this.#focusItem(active.itemId, focused, true);
  }

  async #focusItem(
    itemId: string,
    part: 'item' | 'handle' | undefined,
    preventScroll: boolean,
  ): Promise<void> {
    await tick();
    const item = this.#page.first(ITEM, itemId);
    if (!item) return;
    const target = part === 'handle' ? (this.#page.handlesOf(item)[0] ?? item) : item;
    target.focus({ preventScroll });
  }

  /** The words for what just happened to the drag. */
  #words(kind: 'pickedUp' | 'moved' | 'dropped' | 'cancelled', active: SortableActive): string {
    return sortableWords(this.#options, kind, active);
  }

  /**
   * Puts words in the live region. A live region reads only what changes, so
   * the same words twice in a row are told apart by a space that does not show.
   */
  #say(text: string): void {
    if (text === '') return;
    this.#announcement = text === this.#announcement ? text + NO_BREAK_SPACE : text;
  }
}
