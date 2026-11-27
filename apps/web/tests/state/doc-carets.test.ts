/**
 * The carets of a body as the room's client holds them (`state/doc-carets.ts`):
 * when the reader's own is told to the room, and what is kept of the others'.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { caretTold } from '$lib/state/memory/caret-frame.ts';
import { CARET_WAIT, Carets } from '$lib/state/doc-carets.ts';

const CURSOR = { anchor: { tname: 'default', assoc: 0 }, head: { tname: 'default', assoc: 0 } };

/** Carets of a new document, and what they told the room. */
function carets(takes = true): { held: Carets; told: Uint8Array[]; doc: Y.Doc } {
  const doc = new Y.Doc();
  const told: Uint8Array[] = [];
  const held = new Carets(doc, (update) => {
    if (takes) told.push(update);
    return takes;
  });
  return { held, told, doc };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the reader's caret", () => {
  it('is told once for what was set together, and no more often than every wait', () => {
    const { held, told } = carets();
    held.awareness.setLocalStateField('user', {});
    vi.advanceTimersByTime(CARET_WAIT);
    // No caret yet, and the room knows of none.
    expect(told).toHaveLength(0);
    held.awareness.setLocalStateField('cursor', CURSOR);
    held.awareness.setLocalStateField('cursor', { ...CURSOR, head: { tname: 'x', assoc: 0 } });
    vi.advanceTimersByTime(0);
    expect(told).toHaveLength(1);
    for (let moved = 0; moved < 5; moved += 1) {
      held.awareness.setLocalStateField('cursor', { ...CURSOR, head: { tname: `m${moved}` } });
    }
    vi.advanceTimersByTime(CARET_WAIT - 1);
    expect(told).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(told).toHaveLength(2);
    held.destroy();
  });

  it('is said again when the library renews it, and told gone once', () => {
    const { held, told } = carets();
    held.awareness.setLocalStateField('cursor', CURSOR);
    vi.advanceTimersByTime(CARET_WAIT);
    expect(told).toHaveLength(1);
    // The library renews a state that stays, by setting it again as it is (its clock is the
    // machine's, which the timers of a test do not move).
    held.awareness.setLocalState(held.awareness.getLocalState());
    vi.advanceTimersByTime(CARET_WAIT);
    expect(told).toHaveLength(2);
    held.awareness.setLocalStateField('cursor', null);
    vi.advanceTimersByTime(CARET_WAIT);
    expect(told).toHaveLength(3);
    // Gone, the renewals say nothing more.
    held.awareness.setLocalState(held.awareness.getLocalState());
    vi.advanceTimersByTime(CARET_WAIT);
    expect(told).toHaveLength(3);
    held.destroy();
  });

  it('is told anew once the room can be told again', () => {
    const { held, told } = carets();
    held.awareness.setLocalStateField('cursor', CURSOR);
    vi.advanceTimersByTime(CARET_WAIT);
    held.again();
    vi.advanceTimersByTime(CARET_WAIT);
    expect(told).toHaveLength(2);
    // A room that could not be told is told when it can be.
    const away = carets(false);
    away.held.awareness.setLocalStateField('cursor', CURSOR);
    vi.advanceTimersByTime(CARET_WAIT);
    expect(away.told).toHaveLength(0);
    away.held.destroy();
    held.destroy();
  });

  it('is told gone while the page is hidden, and told anew once it is shown again', () => {
    const { held, told } = carets();
    const visibility = vi.spyOn(document, 'visibilityState', 'get');
    const becomes = (state: DocumentVisibilityState): void => {
      visibility.mockReturnValue(state);
      document.dispatchEvent(new Event('visibilitychange'));
      vi.advanceTimersByTime(CARET_WAIT);
    };
    held.awareness.setLocalStateField('cursor', CURSOR);
    vi.advanceTimersByTime(CARET_WAIT);
    expect(told).toHaveLength(1);
    becomes('hidden');
    expect(told).toHaveLength(2);
    // Told as a state that is gone, though the state still names where the caret was.
    expect(new TextDecoder().decode(told[1])).toContain('null');
    expect(held.awareness.getLocalState()?.cursor).toEqual(CURSOR);
    // Hidden, a renewal says nothing.
    held.awareness.setLocalState(held.awareness.getLocalState());
    vi.advanceTimersByTime(CARET_WAIT);
    expect(told).toHaveLength(2);
    becomes('visible');
    expect(told).toHaveLength(3);
    expect(new TextDecoder().decode(told[2])).toContain('default');
    visibility.mockRestore();
    held.destroy();
    // Let go, it hears nothing more of the page.
    document.dispatchEvent(new Event('visibilitychange'));
    vi.advanceTimersByTime(CARET_WAIT);
    expect(told).toHaveLength(3);
  });

  it('says nothing more once it is let go, and leaves no timer', () => {
    const { held, told } = carets();
    held.awareness.setLocalStateField('cursor', CURSOR);
    held.destroy();
    vi.advanceTimersByTime(60_000);
    expect(told).toHaveLength(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('works when no document is present, or when it goes away before a timer or destroy', () => {
    // Created while a document exists, then the document goes away before the timer fires.
    const removed = vi.spyOn(document, 'removeEventListener');
    const beforeTeardown = carets();
    beforeTeardown.held.awareness.setLocalStateField('cursor', CURSOR);
    vi.stubGlobal('document', undefined);
    vi.advanceTimersByTime(CARET_WAIT);
    expect(beforeTeardown.told).toHaveLength(1);
    beforeTeardown.held.destroy();
    expect(removed).toHaveBeenCalledWith('visibilitychange', expect.any(Function));
    removed.mockRestore();

    // Created when no document exists at all.
    const withoutDoc = carets();
    withoutDoc.held.awareness.setLocalStateField('cursor', CURSOR);
    vi.advanceTimersByTime(CARET_WAIT);
    expect(withoutDoc.told).toHaveLength(1);
    withoutDoc.held.destroy();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("the others' carets", () => {
  it('are taken in as the room tells them, and let go with which telling was the last when the room is left', () => {
    const { held } = carets();
    held.take(caretTold(41, 5, '{"user":{"id":"u"},"cursor":null}'));
    expect(held.awareness.getStates().get(41)).toEqual({ user: { id: 'u' }, cursor: null });
    // Bytes that are no update change nothing.
    held.take(new Uint8Array([9, 9, 9]));
    held.away();
    expect([...held.awareness.getStates().keys()]).toEqual([held.awareness.clientID]);
    // Another room, or another server, counts from one again: its word is taken.
    held.take(caretTold(41, 1, '{"user":{"id":"u"},"cursor":null}'));
    expect(held.awareness.getStates().has(41)).toBe(true);
    held.destroy();
  });

  it('leave nothing behind once they are gone: a room gives every seat that comes in another number', () => {
    const { held } = carets();
    for (let number = 100; number < 150; number += 1) {
      held.take(caretTold(number, 1, '{"user":{"id":"u"},"cursor":null}'));
      held.take(caretTold(number, 2, null));
    }
    expect([...held.awareness.meta.keys()]).toEqual([held.awareness.clientID]);
    // Gone and shown again under its number, it is taken at whatever clock.
    held.take(caretTold(120, 3, '{"user":{"id":"u"},"cursor":null}'));
    expect(held.awareness.getStates().has(120)).toBe(true);
    held.destroy();
  });

  it('are let go by themselves thirty seconds after the last word of them', () => {
    const { held } = carets();
    held.take(caretTold(41, 1, '{"user":{"id":"u"},"cursor":null}'));
    vi.advanceTimersByTime(4000);
    expect(held.awareness.getStates().has(41)).toBe(true);
    // The library looks every three seconds, by the machine's clock: as if the word were old.
    const heard = held.awareness.meta.get(41);
    if (heard) heard.lastUpdated -= 30_000;
    vi.advanceTimersByTime(4000);
    expect(held.awareness.getStates().has(41)).toBe(false);
    held.destroy();
  });
});
