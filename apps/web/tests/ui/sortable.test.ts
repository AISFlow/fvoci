/**
 * `Sortable` against a hand-made page: which of the marked elements an
 * instance takes for its own, and what a drop hands to the caller.
 *
 * The simulated page has no layout, so every element says where it is in a
 * `data-rect="left,top,width,height"` attribute and `getBoundingClientRect`
 * is made to read that. What dragging feels like in a real browser (scrolling,
 * fingers, focus) is tested there: e2e/kanban.spec.ts, kanban-long.spec.ts and
 * fingers.spec.ts.
 *
 * The page: two groups side by side, each a heading and a list of cards, and
 * a bin beside them. Two sortables share the root: one for the cards (no
 * scope), one for the groups (scope "group", held by their headings).
 */
import { afterEach, beforeAll, beforeEach, describe, expect, test, vi } from 'vitest';

import { Sortable, type SortableMove, type SortableOptions } from '$lib/drag/sortable.svelte.ts';
import type { ListOrder } from '$lib/logic/drop-target.ts';

const PAGE = `
  <div id="root" data-drag-list="row" data-drag-scope="group" data-rect="0,0,600,400">
    <section data-drag-item="g1" data-drag-scope="group" data-rect="0,0,200,400">
      <h2 data-drag-handle tabindex="0" data-rect="0,0,200,40">One</h2>
      <ul data-drag-list="g1" data-rect="0,40,200,360">
        <li data-drag-item="a" tabindex="0" data-rect="0,40,200,40">a</li>
        <li data-drag-item="b" tabindex="0" data-rect="0,80,200,40">
          b
          <ul data-drag-list="inner" data-rect="20,90,160,20">
            <li data-drag-item="b-1" data-rect="20,90,160,20">a card within a card</li>
          </ul>
        </li>
        <li data-drag-item="c" tabindex="0" data-rect="0,120,200,40">c</li>
      </ul>
    </section>
    <section data-drag-item="g2" data-drag-scope="group" data-rect="200,0,200,400">
      <h2 data-drag-handle tabindex="0" data-rect="200,0,200,40">Two</h2>
      <ul data-drag-list="g2" data-rect="200,40,200,360">
        <li data-drag-item="d" tabindex="0" data-rect="200,40,200,40">d</li>
      </ul>
    </section>
    <aside data-drag-zone="bin" data-rect="400,0,200,400">bin</aside>
  </div>`;

const CARDS: ListOrder[] = [
  { id: 'g1', items: ['a', 'b', 'c'] },
  { id: 'g2', items: ['d'] },
];
const GROUPS: ListOrder[] = [{ id: 'row', items: ['g1', 'g2'] }];

const MESSAGES: SortableOptions['messages'] = {
  pickedUp: (place) => `up ${place.item} x${place.items} ${place.position}/${place.count}`,
  moved: (place) => `at ${place.item} ${place.position}/${place.count} in ${place.list}`,
  dropped: (place) => `down ${place.item} ${place.position}/${place.count} in ${place.list}`,
  cancelled: (place) => `back ${place.item} ${place.position}/${place.count} in ${place.list}`,
  sorted: () => 'sorted',
  outside: () => 'outside',
  movedToZone: (place) => `over ${place.zone}`,
  droppedInZone: (place) => `down ${place.item} x${place.items} on ${place.zone}`,
};

let root: HTMLElement;
let detach: (() => void)[] = [];

const element = (selector: string): HTMLElement => {
  const found = root.querySelector<HTMLElement>(selector);
  if (!found) throw new Error(`nothing matches ${selector}`);
  return found;
};
const item = (id: string): HTMLElement => element(`[data-drag-item="${id}"]`);
const handle = (id: string): HTMLElement =>
  element(`[data-drag-item="${id}"] > [data-drag-handle]`);

/** A sortable for the cards, attached, and the drops it reported. */
function cards(options: Partial<SortableOptions> = {}): {
  sortable: Sortable;
  drops: SortableMove[];
} {
  const drops: SortableMove[] = [];
  const sortable = new Sortable({
    order: () => CARDS,
    onDrop: (move) => drops.push(move),
    messages: MESSAGES,
    ...options,
  });
  detach.push(sortable.attach(root));
  return { sortable, drops };
}

/** A sortable for the groups, attached, and the drops it reported. */
function groups(): { sortable: Sortable; drops: SortableMove[] } {
  const drops: SortableMove[] = [];
  const sortable = new Sortable({
    scope: 'group',
    axis: 'x',
    order: () => GROUPS,
    onDrop: (move) => drops.push(move),
    messages: MESSAGES,
  });
  detach.push(sortable.attach(root));
  return { sortable, drops };
}

function key(target: EventTarget, name: string): void {
  target.dispatchEvent(
    new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true }),
  );
}

function pointer(target: EventTarget, type: string, x: number, y: number): void {
  target.dispatchEvent(
    new PointerEvent(type, {
      bubbles: true,
      cancelable: true,
      pointerId: 1,
      pointerType: 'mouse',
      isPrimary: true,
      button: 0,
      clientX: x,
      clientY: y,
    }),
  );
}

/** Presses the mouse on an element at a point and moves far enough for a drag to begin. */
function press(target: HTMLElement, x: number, y: number): void {
  pointer(target, 'pointerdown', x, y);
  pointer(window, 'pointermove', x + 10, y);
}

/** The frames asked for and not yet run: the tests run them by hand. */
const frames = new Map<number, FrameRequestCallback>();
let frameIds = 0;

function runFrame(): void {
  const due = [...frames.values()];
  frames.clear();
  for (const callback of due) callback(performance.now());
}

/** Moves the pressed mouse and lets one frame pass: the destination is worked out once per frame. */
function dragTo(x: number, y: number): void {
  pointer(window, 'pointermove', x, y);
  runFrame();
}

function release(x: number, y: number): void {
  pointer(window, 'pointerup', x, y);
}

beforeAll(() => {
  // What the simulated page lacks. Layout comes from the data-rect attributes.
  Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
    const [left = 0, top = 0, width = 0, height = 0] = (this.getAttribute('data-rect') ?? '')
      .split(',')
      .map(Number);
    return new DOMRect(left, top, width, height);
  };
  Element.prototype.scrollIntoView = () => undefined;
  Element.prototype.setPointerCapture = () => undefined;
  Element.prototype.releasePointerCapture = () => undefined;
  Element.prototype.hasPointerCapture = () => false;
  vi.stubGlobal('CSS', { escape: (value: string) => value });
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    frameIds += 1;
    frames.set(frameIds, callback);
    return frameIds;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => {
    frames.delete(id);
  });
});

beforeEach(() => {
  frames.clear();
  document.body.innerHTML = PAGE;
  const found = document.getElementById('root');
  if (!found) throw new Error('the page has no root');
  root = found;
  // The window is larger than the root: outside the root is still on the page.
  window.innerWidth = 1000;
  window.innerHeight = 800;
});

afterEach(() => {
  for (const off of detach) off();
  detach = [];
});

describe('two sortables on one root', () => {
  test('a press on a card starts the cards, a press on a heading the groups, never both', () => {
    const inner = cards();
    const outer = groups();

    press(item('a'), 50, 60);
    expect(inner.sortable.active?.itemId).toBe('a');
    expect(outer.sortable.active).toBeUndefined();
    key(window, 'Escape');
    release(50, 60);
    expect(inner.sortable.active).toBeUndefined();

    press(handle('g1'), 50, 20);
    expect(outer.sortable.active?.itemId).toBe('g1');
    expect(inner.sortable.active).toBeUndefined();
    key(window, 'Escape');
    release(50, 20);
  });

  test('a group is held by its heading only', () => {
    const inner = cards();
    const outer = groups();
    // On the group, beside its heading and its cards: the list below the last card.
    press(element('[data-drag-list="g1"]'), 50, 300);
    expect(outer.sortable.active).toBeUndefined();
    expect(inner.sortable.active).toBeUndefined();
    release(60, 300);
  });

  test('a heading of a card within is not the heading of the group around it', () => {
    item('a').insertAdjacentHTML('beforeend', '<span data-drag-handle>grip</span>');
    const inner = cards();
    const outer = groups();
    // The group is still held by its own heading, and the card now only by its grip.
    press(handle('g1'), 50, 20);
    expect(outer.sortable.active?.itemId).toBe('g1');
    key(window, 'Escape');
    release(50, 20);
    press(item('a'), 50, 60);
    expect(inner.sortable.active).toBeUndefined();
    release(60, 60);
    press(element('[data-drag-item="a"] > [data-drag-handle]'), 50, 60);
    expect(inner.sortable.active?.itemId).toBe('a');
    key(window, 'Escape');
    release(50, 60);
  });

  test('Space on a card picks up the card, Space on a heading the group', () => {
    const inner = cards();
    const outer = groups();

    key(item('a'), ' ');
    expect(inner.sortable.active).toMatchObject({ itemId: 'a', mode: 'keyboard' });
    expect(outer.sortable.active).toBeUndefined();
    key(window, 'Escape');

    key(handle('g2'), ' ');
    expect(outer.sortable.active).toMatchObject({ itemId: 'g2', mode: 'keyboard' });
    expect(inner.sortable.active).toBeUndefined();
    key(window, 'Escape');
  });

  test('a group dragged along the row lands among the groups, and the cards are not asked', () => {
    const inner = cards();
    const outer = groups();
    press(handle('g1'), 50, 20);
    // Past the middle of the second group.
    dragTo(350, 20);
    release(350, 20);
    expect(outer.drops).toEqual([
      {
        itemId: 'g1',
        itemIds: ['g1'],
        from: { listId: 'row', index: 0 },
        to: { kind: 'index', listId: 'row', index: 1 },
      },
    ]);
    expect(inner.drops).toEqual([]);
  });

  test('the keyboard moves a group along the row with left and right', () => {
    const outer = groups();
    key(handle('g1'), ' ');
    key(window, 'ArrowDown');
    expect(outer.sortable.active?.gap).toEqual({ listId: 'row', index: 0 });
    key(window, 'ArrowRight');
    expect(outer.sortable.active?.gap).toEqual({ listId: 'row', index: 1 });
    key(window, 'Enter');
    expect(outer.drops.map((move) => move.to)).toEqual([
      { kind: 'index', listId: 'row', index: 1 },
    ]);
  });

  test('on a page that runs right to left the row is read from the right', () => {
    root.style.direction = 'rtl';
    // Mirrored: the first group is at the right.
    item('g1').setAttribute('data-rect', '200,0,200,400');
    handle('g1').setAttribute('data-rect', '200,0,200,40');
    item('g2').setAttribute('data-rect', '0,0,200,400');
    const outer = groups();

    press(handle('g1'), 250, 20);
    // To the left of the middle of the second group: past it.
    dragTo(50, 20);
    expect(outer.sortable.active?.gap).toEqual({ listId: 'row', index: 1 });
    dragTo(150, 20);
    expect(outer.sortable.active?.gap).toEqual({ listId: 'row', index: 0 });
    key(window, 'Escape');
    release(150, 20);

    key(handle('g1'), ' ');
    key(window, 'ArrowLeft');
    expect(outer.sortable.active?.gap).toEqual({ listId: 'row', index: 1 });
    key(window, 'ArrowRight');
    expect(outer.sortable.active?.gap).toEqual({ listId: 'row', index: 0 });
    key(window, 'Escape');
  });
});

describe('the items of a list', () => {
  test('are those whose nearest list it is: the cards of a list within a card do not count', () => {
    const { drops } = cards();
    press(item('d'), 250, 60);
    // Below c, the last of the three cards of the first list.
    dragTo(100, 300);
    release(100, 300);
    expect(drops.map((move) => move.to)).toEqual([{ kind: 'index', listId: 'g1', index: 3 }]);
  });

  test('a card is placed by the middles of the cards', () => {
    const { sortable, drops } = cards();
    press(item('d'), 250, 60);
    dragTo(100, 95);
    expect(sortable.active?.gap).toEqual({ listId: 'g1', index: 1 });
    dragTo(100, 105);
    expect(sortable.active?.gap).toEqual({ listId: 'g1', index: 2 });
    release(100, 105);
    expect(drops).toEqual([
      {
        itemId: 'd',
        itemIds: ['d'],
        from: { listId: 'g2', index: 0 },
        to: { kind: 'index', listId: 'g1', index: 2 },
      },
    ]);
  });
});

describe('outside the root', () => {
  test('the drag says so, and a release there is a cancel', () => {
    const { sortable, drops } = cards();
    press(item('a'), 50, 60);
    dragTo(250, 300);
    expect(sortable.active).toMatchObject({ outside: false, to: { kind: 'index', listId: 'g2' } });
    expect(sortable.notice).toBe('');

    dragTo(700, 300);
    expect(sortable.active).toMatchObject({
      outside: true,
      to: undefined,
      gap: { listId: 'g1', index: 0 },
    });
    expect(sortable.notice).toBe('outside');

    release(700, 300);
    expect(sortable.active).toBeUndefined();
    expect(drops).toEqual([]);
    expect(sortable.announcement).toBe('back a 1/3 in g1');
    expect(sortable.notice).toBe('');
  });

  test('back inside the drag goes on', () => {
    const { sortable, drops } = cards();
    press(item('a'), 50, 60);
    dragTo(700, 300);
    dragTo(250, 300);
    expect(sortable.active?.outside).toBe(false);
    release(250, 300);
    expect(drops.map((move) => move.to)).toEqual([{ kind: 'index', listId: 'g2', index: 1 }]);
  });

  test('the part of the root that is off the window is outside as well', () => {
    window.innerWidth = 300;
    const { sortable } = cards();
    press(item('a'), 50, 60);
    dragTo(350, 100);
    expect(sortable.active?.outside).toBe(true);
    key(window, 'Escape');
    release(350, 100);
  });
});

describe('a view whose lists place items themselves', () => {
  const sorted = (extra: Partial<SortableOptions> = {}) =>
    cards({ placement: () => 'list', ...extra });

  test('the pointer chooses the list only; the gap is where the order puts the card', () => {
    const sortedIndex = vi.fn<NonNullable<SortableOptions['sortedIndex']>>(() => 1);
    const { sortable, drops } = sorted({ sortedIndex });
    press(item('d'), 250, 60);
    expect(sortable.active).toMatchObject({
      placement: 'list',
      to: { kind: 'list', listId: 'g2' },
    });
    expect(sortable.notice).toBe('sorted');
    expect(sortable.announcement).toBe('sorted');

    dragTo(100, 45);
    expect(sortable.active?.gap).toEqual({ listId: 'g1', index: 1 });
    dragTo(100, 300);
    expect(sortable.active?.gap).toEqual({ listId: 'g1', index: 1 });
    expect(sortedIndex).toHaveBeenCalledWith('d', 'g1', ['d']);
    release(100, 300);

    // The move names the list and has no index to read.
    expect(drops).toEqual([
      {
        itemId: 'd',
        itemIds: ['d'],
        from: { listId: 'g2', index: 0 },
        to: { kind: 'list', listId: 'g1' },
      },
    ]);
    expect(drops[0]?.to).not.toHaveProperty('index');
  });

  test('without anyone to say where the order puts it, the gap is at the end', () => {
    const { sortable } = sorted();
    press(item('d'), 250, 60);
    dragTo(100, 45);
    expect(sortable.active?.gap).toEqual({ listId: 'g1', index: 3 });
    key(window, 'Escape');
    release(100, 45);
  });

  test('within its own list a card does not move, and a drop reports nothing', () => {
    const { sortable, drops } = sorted({ sortedIndex: () => 0 });
    press(item('a'), 50, 60);
    dragTo(100, 300);
    expect(sortable.active).toMatchObject({
      to: { kind: 'list', listId: 'g1' },
      gap: { listId: 'g1', index: 0 },
    });
    release(100, 300);
    expect(drops).toEqual([]);
  });

  test('the keyboard: up and down stay put and say why; left and right change the list', () => {
    const { sortable, drops } = sorted({ sortedIndex: () => 0 });
    key(item('b'), ' ');
    expect(sortable.announcement).toBe('up b x1 2/3 sorted');

    key(window, 'ArrowDown');
    expect(sortable.active?.gap).toEqual({ listId: 'g1', index: 1 });
    expect(sortable.announcement).toBe('sorted');
    // Asked again, said again: the live region must see a change.
    key(window, 'ArrowUp');
    expect(sortable.announcement).not.toBe('sorted');
    expect(sortable.announcement.trim()).toBe('sorted');
    key(window, 'End');
    expect(sortable.active?.gap).toEqual({ listId: 'g1', index: 1 });

    key(window, 'ArrowRight');
    expect(sortable.active).toMatchObject({
      to: { kind: 'list', listId: 'g2' },
      gap: { listId: 'g2', index: 0 },
    });
    expect(sortable.announcement).toBe('at b 1/2 in g2');
    key(window, 'Enter');
    expect(drops.map((move) => move.to)).toEqual([{ kind: 'list', listId: 'g2' }]);
  });

  test('the placement is asked for when a card is picked up', () => {
    let placement: 'free' | 'list' = 'free';
    const { sortable } = cards({ placement: () => placement });
    key(item('a'), ' ');
    expect(sortable.active?.placement).toBe('free');
    key(window, 'Escape');
    placement = 'list';
    key(item('a'), ' ');
    expect(sortable.active?.placement).toBe('list');
    key(window, 'Escape');
  });
});

describe('a zone', () => {
  test('a card released on it is reported with the zone, and has no gap meanwhile', () => {
    const { sortable, drops } = cards();
    press(item('a'), 50, 60);
    dragTo(500, 200);
    expect(sortable.active).toMatchObject({ to: { kind: 'zone', zoneId: 'bin' }, gap: undefined });
    expect(sortable.active?.outside).toBe(false);
    release(500, 200);
    expect(drops).toEqual([
      {
        itemId: 'a',
        itemIds: ['a'],
        from: { listId: 'g1', index: 0 },
        to: { kind: 'zone', zoneId: 'bin' },
      },
    ]);
    expect(sortable.announcement).toBe('down a x1 on bin');
  });

  test('a zone of another scope is not this instance’s to hit', () => {
    element('[data-drag-zone]').setAttribute('data-drag-scope', 'group');
    const { sortable } = cards();
    press(item('a'), 50, 60);
    dragTo(500, 200);
    expect(sortable.active?.to).toMatchObject({ kind: 'index', listId: 'g2' });
    key(window, 'Escape');
    release(500, 200);
  });

  test('the keyboard reaches the zones the caller names, past the last list, and comes back', () => {
    const { sortable, drops } = cards({ zones: () => ['bin'] });
    key(item('c'), ' ');
    key(window, 'ArrowRight');
    expect(sortable.active?.to).toEqual({ kind: 'index', listId: 'g2', index: 1 });
    key(window, 'ArrowRight');
    expect(sortable.active?.to).toEqual({ kind: 'zone', zoneId: 'bin' });
    expect(sortable.announcement).toBe('over bin');
    key(window, 'ArrowLeft');
    expect(sortable.active?.to).toEqual({ kind: 'index', listId: 'g2', index: 1 });
    key(window, 'ArrowRight');
    key(window, ' ');
    expect(drops.map((move) => move.to)).toEqual([{ kind: 'zone', zoneId: 'bin' }]);
  });

  test('the keyboard knows no zones unless the caller names them', () => {
    const { sortable } = cards();
    key(item('d'), ' ');
    key(window, 'ArrowRight');
    expect(sortable.active?.to).toEqual({ kind: 'index', listId: 'g2', index: 0 });
    key(window, 'Escape');
  });
});

describe('a selection', () => {
  test('grabbing one of the selected carries them all, in the order of the data', () => {
    const { sortable, drops } = cards({ selection: () => ['c', 'a'] });
    press(item('c'), 50, 140);
    expect(sortable.active).toMatchObject({
      itemId: 'c',
      itemIds: ['a', 'c'],
      from: { listId: 'g1', index: 1 },
    });
    dragTo(250, 300);
    release(250, 300);
    expect(drops).toEqual([
      {
        itemId: 'c',
        itemIds: ['a', 'c'],
        from: { listId: 'g1', index: 1 },
        to: { kind: 'index', listId: 'g2', index: 1 },
      },
    ]);
  });

  test('the place counts the list without any of the carried cards', () => {
    const { sortable } = cards({ selection: () => ['a', 'b'] });
    press(item('a'), 50, 60);
    // Below c: with a and b on their way, c is the only card left before the place.
    dragTo(100, 300);
    expect(sortable.active?.gap).toEqual({ listId: 'g1', index: 1 });
    key(window, 'Escape');
    release(100, 300);
  });

  test('grabbing a card that is not selected carries it alone', () => {
    const { sortable } = cards({ selection: () => ['a', 'c'] });
    key(item('b'), ' ');
    expect(sortable.active?.itemIds).toEqual(['b']);
    key(window, 'Escape');
  });

  test('the keyboard carries the selection, and says how many', () => {
    const { sortable, drops } = cards({ selection: () => ['a', 'c'] });
    key(item('a'), ' ');
    expect(sortable.announcement).toBe('up a x2 1/3');
    key(window, 'ArrowRight');
    key(window, 'Enter');
    expect(drops.map((move) => [move.itemIds, move.to])).toEqual([
      [['a', 'c'], { kind: 'index', listId: 'g2', index: 0 }],
    ]);
  });
});
