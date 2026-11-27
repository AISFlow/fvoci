/**
 * Whether the WebSocket still carries (`lib/api/live-heartbeat.ts`), against a
 * stand-in socket: a `ping` nothing answers in time lets the connection go
 * and opens another, anything that comes is an answer, and a page whose
 * timers were held back (a tab in the background) is not cut for that.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { LONGEST_PONG_WAIT, PING_EVERY, PONG_WAIT } from '$lib/api/live-heartbeat.ts';
import { LiveChanges } from '$lib/api/live.ts';
import { SharedPresence } from '$lib/api/shared-presence.ts';
import type { Change } from '$lib/state/index.ts';

class FakeSocket extends EventTarget {
  static made: FakeSocket[] = [];
  readonly sent: unknown[] = [];
  closed = false;

  constructor() {
    super();
    FakeSocket.made.push(this);
  }

  send(text: string): void {
    this.sent.push(JSON.parse(text));
  }

  close(): void {
    this.closed = true;
  }

  open(): void {
    this.dispatchEvent(new Event('open'));
  }

  message(data: unknown): void {
    this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(data) }));
  }

  pings(): number {
    return this.sent.filter((sent) => (sent as { type?: string }).type === 'ping').length;
  }
}

const PONG = { type: 'pong' };

let stops: (() => void)[] = [];

beforeEach(() => {
  FakeSocket.made = [];
  vi.stubGlobal('WebSocket', FakeSocket);
  vi.useFakeTimers();
});

afterEach(() => {
  for (const stop of stops) stop();
  stops = [];
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** A page that listens to a board, its connection open: what it was told, and the socket. */
async function connected(): Promise<{ told: Change[]; socket: FakeSocket }> {
  const live = new LiveChanges('ws://here/ws', { members: () => Promise.resolve([]) });
  live.enter('t');
  const told: Change[] = [];
  stops.push(live.watch('b1', (change) => told.push(change)));
  await vi.advanceTimersByTimeAsync(0);
  const socket = FakeSocket.made[0];
  if (!socket) throw new Error('no socket');
  socket.open();
  return { told, socket };
}

/** Moves the clock on without running the timers, as a page whose timers are held back sees it. */
function sleptFor(milliseconds: number): void {
  vi.setSystemTime(Date.now() + milliseconds);
}

describe('a connection that may have died', () => {
  it('asks after a while, and lets go of a connection nothing answers to open another', async () => {
    const { told, socket } = await connected();
    await vi.advanceTimersByTimeAsync(PING_EVERY - 1);
    expect(socket.pings()).toBe(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(socket.sent.at(-1)).toEqual({ type: 'ping' });
    await vi.advanceTimersByTimeAsync(PONG_WAIT - 1);
    expect(socket.closed).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(socket.closed).toBe(true);
    // Opened again after the first wait, and everyone reads again.
    await vi.advanceTimersByTimeAsync(500);
    const again = FakeSocket.made[1];
    expect(again).toBeDefined();
    again?.open();
    expect(told).toEqual([{ kind: 'resync' }]);
    // What the old one says now is not heard, and it asks nothing more.
    socket.message({ type: 'clientConfigChanged' });
    await vi.advanceTimersByTimeAsync(PING_EVERY * 3);
    expect(socket.pings()).toBe(1);
  });

  it('keeps a connection that answers, and asks again a while after each answer', async () => {
    const { socket } = await connected();
    for (let round = 1; round <= 3; round += 1) {
      await vi.advanceTimersByTimeAsync(PING_EVERY);
      expect(socket.pings()).toBe(round);
      await vi.advanceTimersByTimeAsync(PONG_WAIT - 1000);
      socket.message(PONG);
    }
    await vi.advanceTimersByTimeAsync(PONG_WAIT * 2);
    expect(socket.closed).toBe(false);
    expect(FakeSocket.made).toHaveLength(1);
  });

  it('takes anything that comes as an answer', async () => {
    const { socket } = await connected();
    await vi.advanceTimersByTimeAsync(PING_EVERY);
    socket.message({ type: 'clientConfigChanged', clientConfig: {} });
    socket.dispatchEvent(new MessageEvent('message', { data: new ArrayBuffer(0) }));
    await vi.advanceTimersByTimeAsync(PONG_WAIT * 2);
    expect(socket.closed).toBe(false);
  });

  it('does not cut a page whose timers were held back, and asks once more instead', async () => {
    const { socket } = await connected();
    await vi.advanceTimersByTimeAsync(PING_EVERY);
    expect(socket.pings()).toBe(1);
    // The tab went to the background: its wait ends a minute late. The answer
    // could not be heard meanwhile either.
    sleptFor(60_000);
    await vi.advanceTimersByTimeAsync(PONG_WAIT);
    expect(socket.closed).toBe(false);
    expect(socket.pings()).toBe(2);
    socket.message(PONG);
    // Late again, and answered: still there.
    await vi.advanceTimersByTimeAsync(PING_EVERY);
    sleptFor(45_000);
    await vi.advanceTimersByTimeAsync(PONG_WAIT);
    socket.message(PONG);
    await vi.advanceTimersByTimeAsync(PONG_WAIT * 2);
    expect(socket.closed).toBe(false);
    expect(FakeSocket.made).toHaveLength(1);
  });

  it('lets go once the PING asked again is not answered either', async () => {
    const { socket } = await connected();
    await vi.advanceTimersByTimeAsync(PING_EVERY);
    sleptFor(60_000);
    await vi.advanceTimersByTimeAsync(PONG_WAIT);
    expect(socket.closed).toBe(false);
    await vi.advanceTimersByTimeAsync(PONG_WAIT);
    expect(socket.closed).toBe(true);
  });

  it('waits twice as long on the connection after one let go unanswered, and as first once answered', async () => {
    const { socket } = await connected();
    // Let go unanswered: a large message on a slow link looks so too.
    await vi.advanceTimersByTimeAsync(PING_EVERY + PONG_WAIT);
    expect(socket.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(500);
    const again = FakeSocket.made[1];
    if (!again) throw new Error('no second socket');
    again.open();
    await vi.advanceTimersByTimeAsync(PING_EVERY + PONG_WAIT);
    expect(again.closed).toBe(false);
    await vi.advanceTimersByTimeAsync(PONG_WAIT - 1000);
    again.message(PONG);
    // Answered: the next waits as the first did.
    await vi.advanceTimersByTimeAsync(PING_EVERY + PONG_WAIT);
    expect(again.closed).toBe(true);
  });

  it('waits a minute at the longest', async () => {
    const { socket } = await connected();
    let current = socket;
    let wait = PONG_WAIT;
    for (let cut = 1; cut <= 5; cut += 1) {
      await vi.advanceTimersByTimeAsync(PING_EVERY + wait - 1);
      expect(current.closed).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      expect(current.closed).toBe(true);
      await vi.advanceTimersByTimeAsync(30_000);
      const next = FakeSocket.made.at(-1);
      if (!next || next === current) throw new Error('no new socket');
      next.open();
      current = next;
      wait = Math.min(wait * 2, LONGEST_PONG_WAIT);
    }
    expect(wait).toBe(LONGEST_PONG_WAIT);
  });

  it('sends its next PING before the server would close the connection as quiet', () => {
    // The server closes a connection that sends nothing for 90 seconds
    // (`QUIET_LIMIT` in crates/server/src/realtime/quiet.rs).
    expect(PING_EVERY + LONGEST_PONG_WAIT).toBeLessThan(90_000);
  });

  it('asks nothing once nobody listens', async () => {
    const { socket } = await connected();
    for (const stop of stops) stop();
    stops = [];
    expect(socket.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(PING_EVERY * 5);
    expect(socket.pings()).toBe(0);
    expect(FakeSocket.made).toHaveLength(1);
  });
});

describe('the visit of a shared board', () => {
  it('lets go of a connection nothing answers, and opens another', async () => {
    const presence = new SharedPresence('', 'shared-board', 'read-token');
    presence.setPresence({ workspaceId: 'shared', containerId: 'shared-board', cardId: '' });
    const first = FakeSocket.made[0];
    if (!first) throw new Error('a visit opens a socket');
    first.open();
    await vi.advanceTimersByTimeAsync(PING_EVERY);
    expect(first.pings()).toBe(1);
    await vi.advanceTimersByTimeAsync(PONG_WAIT);
    expect(first.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(500);
    expect(FakeSocket.made).toHaveLength(2);
    presence.setPresence(null);
  });
});
