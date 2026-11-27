/** Reads only the elements owned by one sortable's scope. */
import type { Destination } from '$lib/logic/drop-target.ts';
import type { ScrollBox } from './auto-scroll.ts';
import type { SortableOptions } from './sortable-types.ts';

export const ITEM = 'data-drag-item';
export const LIST = 'data-drag-list';
export const ZONE = 'data-drag-zone';
const SCROLL = 'data-drag-scroll';
export const HANDLE = 'data-drag-handle';
export const CARRIED = 'data-drag-carried';
/** On an item of a list that draws only some of its items: its index among the items that stay. */
const AT = 'data-drag-at';
const SCOPE = 'data-drag-scope';

/** Controls inside an item keep their own presses. */
const CONTROLS =
  'input, textarea, select, button, a[href], [contenteditable]:not([contenteditable="false"])';

/**
 * The index in the whole list of a place among the items drawn: before the
 * `index`th of `items`, or after the last. A list that draws only some of its
 * items says where each is (`data-drag-at`); without that, the items drawn
 * are the list.
 */
export function placeAmong(items: readonly HTMLElement[], index: number): number {
  const next = items[index]?.getAttribute(AT) ?? null;
  if (next !== null) return Number(next);
  const previous = items[index - 1]?.getAttribute(AT) ?? null;
  if (previous !== null) return Number(previous) + 1;
  return index;
}

export class SortableDom {
  root: HTMLElement | undefined;
  readonly #options: Pick<SortableOptions, 'scope'>;

  constructor(options: Pick<SortableOptions, 'scope'>) {
    this.#options = options;
  }

  /** Matches the elements of this instance that carry `attribute`, with `value` when given. */
  #selector(attribute: string, value?: string): string {
    const marked = value === undefined ? `[${attribute}]` : `[${attribute}="${CSS.escape(value)}"]`;
    const scope = this.#options.scope;
    return scope === undefined
      ? `${marked}:not([${SCOPE}])`
      : `${marked}[${SCOPE}="${CSS.escape(scope)}"]`;
  }

  /**
   * This instance's elements that carry any of the attributes, the root
   * included, in the order of the document. One walk through the root,
   * however many attributes are asked for.
   */
  all(...attributes: string[]): HTMLElement[] {
    const root = this.root;
    if (!root) return [];
    const selector = attributes.map((attribute) => this.#selector(attribute)).join(', ');
    const inside = [...root.querySelectorAll<HTMLElement>(selector)];
    return root.matches(selector) ? [root, ...inside] : inside;
  }

  /** The first of this instance's elements with that attribute and value: the root, or one in it. */
  first(attribute: string, value?: string): HTMLElement | undefined {
    const root = this.root;
    if (!root) return undefined;
    const selector = this.#selector(attribute, value);
    return root.matches(selector) ? root : (root.querySelector<HTMLElement>(selector) ?? undefined);
  }

  /**
   * The items of a list: those of this scope whose nearest list is this one,
   * not those of a list within. `nested` says whether there is a list within
   * at all: this runs on every frame of a drag, over every item of a long
   * list, and the walk up from each item is only made when it has to be.
   */
  itemsOf(list: HTMLElement, nested: boolean): HTMLElement[] {
    const items = [...list.querySelectorAll<HTMLElement>(this.#selector(ITEM))];
    if (!nested) return items;
    const lists = this.#selector(LIST);
    return items.filter((item) => item.closest(lists) === list);
  }

  /** The handles of an item: not those of items within it. */
  handlesOf(item: HTMLElement): HTMLElement[] {
    return [...item.querySelectorAll<HTMLElement>(`[${HANDLE}]`)].filter(
      (handle) => handle.closest(`[${ITEM}]`) === item,
    );
  }

  isMine(element: Element): boolean {
    return (element.getAttribute(SCOPE) ?? undefined) === this.#options.scope;
  }

  /** Right to left: a row of items runs against the x coordinate, and the arrow keys swap. */
  rtl(): boolean {
    return this.root !== undefined && getComputedStyle(this.root).direction === 'rtl';
  }

  /**
   * The item a press on `target` would drag, if it is this instance's. The
   * press belongs to the innermost item around it, of whatever scope.
   */
  itemAt(target: EventTarget | null): HTMLElement | undefined {
    if (!(target instanceof Element)) return undefined;
    const item = target.closest<HTMLElement>(`[${ITEM}]`);
    if (!item || !this.root?.contains(item) || !this.isMine(item)) return undefined;
    const control = target.closest(CONTROLS);
    if (control && control !== item && item.contains(control)) return undefined;
    const handles = this.handlesOf(item);
    if (handles.length > 0 && !handles.some((handle) => handle.contains(target))) return undefined;
    return item;
  }

  /** Whether the focus is on the item or on its handle now. */
  focusOn(item: HTMLElement): 'item' | 'handle' | undefined {
    const focused = document.activeElement;
    if (focused === item) return 'item';
    return this.handlesOf(item).some((handle) => handle === focused) ? 'handle' : undefined;
  }

  /**
   * The boxes that may scroll now: those around the list or zone the item
   * would land in. None while the pointer is outside the root: there a
   * release cancels, and a board that kept scrolling would say otherwise.
   */
  scrollBoxes(to: Destination | undefined): ScrollBox[] {
    const root = this.root;
    if (!root || !to) return [];
    const boxes: ScrollBox[] = [];
    let element = to.kind === 'zone' ? this.first(ZONE, to.zoneId) : this.first(LIST, to.listId);
    while (element && root.contains(element)) {
      const axis = element.getAttribute(SCROLL);
      if (axis === 'x' || axis === 'y') boxes.push({ element, axis });
      element = element.parentElement ?? undefined;
    }
    return boxes;
  }
}
