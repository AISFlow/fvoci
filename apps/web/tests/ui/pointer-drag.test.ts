/**
 * `pointerDrag` on its own: when a press becomes a drag, what the session
 * hears, and whether the click after it goes through. Above all the two
 * ways a Gantt chart and a calendar use it: taken at once (`immediate`), and
 * with Escape left alone (`escapeCancels: false`).
 *
 * jsdom has no pointer capture; the root records it instead. How a drag
 * feels in a real browser is tested there (e2e/).
 */
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import {
  pointerDrag,
  type PointerDragHandle,
  type PointerDragManner,
} from '$lib/drag/pointer-drag.ts';
import { DRAG_SETTINGS } from '$lib/drag/settings.ts';
import type { Point } from '$lib/logic/drop-target.ts';

let root: HTMLElement;
let item: HTMLButtonElement;
let handle: PointerDragHandle | undefined;
let heard: string[];
let clicks: number;
let captured: Set<number>;

function pointer(
  type: string,
  at: Point,
  init: { pointerType?: string; target?: EventTarget } = {},
): void {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: at.x,
    clientY: at.y,
  });
  Object.defineProperties(event, {
    pointerId: { value: 1 },
    pointerType: { value: init.pointerType ?? 'mouse' },
    isPrimary: { value: true },
  });
  (init.target ?? item).dispatchEvent(event);
}

/** A press and release where nothing moved, as the browser sends it: pointerup, then click. */
function release(at: Point, pointerType = 'mouse'): void {
  pointer('pointerup', at, { pointerType });
  item.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

function attach(manner: PointerDragManner = {}): void {
  handle = pointerDrag(
    root,
    () => ({
      start: (point) => heard.push(`start ${point.x}`),
      move: (point) => heard.push(`move ${point.x}`),
      drop: (point) => heard.push(`drop ${point.x}`),
      cancel: () => heard.push('cancel'),
    }),
    manner,
  );
}

beforeEach(() => {
  document.body.innerHTML = '<div id="root"><button id="item">a bar</button></div>';
  root = document.querySelector<HTMLElement>('#root') ?? document.body;
  item = document.querySelector<HTMLButtonElement>('#item') ?? document.createElement('button');
  heard = [];
  clicks = 0;
  captured = new Set();
  item.addEventListener('click', () => (clicks += 1));
  root.setPointerCapture = (id: number) => captured.add(id);
  root.releasePointerCapture = (id: number) => captured.delete(id);
  root.hasPointerCapture = (id: number) => captured.has(id);
});

afterEach(() => {
  handle?.destroy();
  handle = undefined;
  vi.useRealTimers();
});

const far = DRAG_SETTINGS.mouseDistance;

describe('a mouse, as a list drags', () => {
  test('begins after a few pixels, where the pointer then is, and swallows the click', () => {
    attach();
    pointer('pointerdown', { x: 0, y: 0 });
    pointer('pointermove', { x: far - 1, y: 0 });
    expect(heard).toEqual([]);
    pointer('pointermove', { x: far, y: 0 });
    pointer('pointermove', { x: 20, y: 0 });
    expect(captured.has(1)).toBe(true);
    release({ x: 20, y: 0 });
    expect(heard).toEqual([`start ${far}`, 'move 20', 'drop 20']);
    expect(clicks).toBe(0);
  });

  test('Escape cancels the drag, and the button going up is no click', () => {
    attach();
    pointer('pointerdown', { x: 0, y: 0 });
    pointer('pointermove', { x: 10, y: 0 });
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true }));
    release({ x: 10, y: 0 });
    expect(heard).toEqual(['start 10', 'cancel']);
    expect(clicks).toBe(0);
  });
});

describe('a mouse, taken at once', () => {
  test('starts at the press, where it was pressed, without taking the pointer yet', () => {
    attach({ immediate: true });
    pointer('pointerdown', { x: 3, y: 0 });
    expect(heard).toEqual(['start 3']);
    expect(captured.has(1)).toBe(false);
  });

  test('a press let go before it went the few pixels is a click: cancelled, and the click goes through', () => {
    attach({ immediate: true });
    pointer('pointerdown', { x: 0, y: 0 });
    pointer('pointermove', { x: far - 1, y: 0 });
    // Not captured: the browser gives the click to what was pressed, not to the root.
    expect(captured.has(1)).toBe(false);
    release({ x: far - 1, y: 0 });
    expect(heard).toEqual(['start 0', 'cancel']);
    expect(clicks).toBe(1);
  });

  test('the moves follow once it has gone the few pixels, and the drop swallows the click', () => {
    attach({ immediate: true });
    pointer('pointerdown', { x: 0, y: 0 });
    pointer('pointermove', { x: 1, y: 0 });
    pointer('pointermove', { x: far, y: 0 });
    expect(captured.has(1)).toBe(true);
    pointer('pointermove', { x: 2, y: 0 });
    release({ x: 2, y: 0 });
    // Back near the press it is still a drag: it went far enough once.
    expect(heard).toEqual(['start 0', `move ${far}`, 'move 2', 'drop 2']);
    expect(clicks).toBe(0);
  });

  test('a finger still rests first', () => {
    vi.useFakeTimers();
    attach({ immediate: true });
    pointer('pointerdown', { x: 0, y: 0 }, { pointerType: 'touch' });
    expect(heard).toEqual([]);
    vi.advanceTimersByTime(DRAG_SETTINGS.touchDelay);
    expect(heard).toEqual(['start 0']);
    // Rested: a move of a pixel is a move already.
    pointer('pointermove', { x: 1, y: 0 }, { pointerType: 'touch' });
    release({ x: 1, y: 0 }, 'touch');
    expect(heard).toEqual(['start 0', 'move 1', 'drop 1']);
  });

  test('a finger that moves before resting scrolls, and nothing starts', () => {
    vi.useFakeTimers();
    attach({ immediate: true });
    pointer('pointerdown', { x: 0, y: 0 }, { pointerType: 'touch' });
    pointer('pointermove', { x: DRAG_SETTINGS.touchTolerance + 1, y: 0 }, { pointerType: 'touch' });
    vi.advanceTimersByTime(DRAG_SETTINGS.touchDelay);
    expect(heard).toEqual([]);
  });
});

describe('Escape left alone', () => {
  test('is the default only where asked for: left out, Escape cancels', () => {
    attach({ immediate: true, escapeCancels: undefined });
    pointer('pointerdown', { x: 0, y: 0 });
    pointer('pointermove', { x: 10, y: 0 });
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true }));
    expect(heard).toEqual(['start 0', 'move 10', 'cancel']);
  });

  test('does not cancel a drag, and nothing else on the page hears it meanwhile', () => {
    attach({ immediate: true, escapeCancels: false });
    const elsewhere = vi.fn();
    document.addEventListener('keydown', elsewhere);
    pointer('pointerdown', { x: 0, y: 0 });
    pointer('pointermove', { x: 10, y: 0 });
    item.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }),
    );
    release({ x: 12, y: 0 });
    document.removeEventListener('keydown', elsewhere);
    expect(heard).toEqual(['start 0', 'move 10', 'drop 12']);
    expect(elsewhere).not.toHaveBeenCalled();
  });

  test('reaches the page again once the drag is over', () => {
    attach({ immediate: true, escapeCancels: false });
    pointer('pointerdown', { x: 0, y: 0 });
    pointer('pointermove', { x: 10, y: 0 });
    release({ x: 10, y: 0 });
    const elsewhere = vi.fn();
    document.addEventListener('keydown', elsewhere);
    item.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    document.removeEventListener('keydown', elsewhere);
    expect(elsewhere).toHaveBeenCalledOnce();
  });
});
