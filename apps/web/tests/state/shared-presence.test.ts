import { afterEach, expect, it, vi } from 'vitest';
import { SharedPresence } from '$lib/api/shared-presence.ts';

class Socket extends EventTarget {
  static made: Socket[] = [];
  readonly sent: Record<string, unknown>[] = [];
  readonly url: string;
  constructor(url: string) {
    super();
    this.url = url;
    Socket.made.push(this);
  }
  send(text: string): void {
    this.sent.push(JSON.parse(text) as Record<string, unknown>);
  }
  close(): void {
    this.dispatchEvent(new Event('close'));
  }
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it('a shared visit sends only its board and token, renews after reconnect, and stops on departure', () => {
  vi.useFakeTimers();
  vi.stubGlobal('WebSocket', Socket);
  Socket.made = [];
  const live = new SharedPresence('', 'shared-board', 'read-token');
  const hear = vi.fn();
  const stop = live.watchPresence('shared-board', hear);
  expect(Socket.made).toHaveLength(0);
  live.setPresence({ workspaceId: 'shared', containerId: 'shared-board', cardId: 'card' });
  const first = Socket.made[0];
  if (!first) throw new Error('a visit opens a socket');
  first.dispatchEvent(new Event('open'));
  expect(first.sent).toEqual([
    {
      type: 'setPresence',
      workspaceId: '',
      containerId: 'shared-board',
      cardId: 'card',
      readToken: 'read-token',
    },
  ]);
  first.dispatchEvent(
    new MessageEvent('message', {
      data: JSON.stringify({ type: 'presenceChanged', people: [{ userId: 'never-shown' }] }),
    }),
  );
  expect(hear).not.toHaveBeenCalled();
  vi.advanceTimersByTime(30_000);
  // Renewed once, and asked once whether the connection carries (`live-heartbeat.ts`).
  expect(first.sent.map((sent) => sent.type)).toEqual(['setPresence', 'ping', 'setPresence']);
  first.close();
  vi.advanceTimersByTime(500);
  const second = Socket.made[1];
  if (!second) throw new Error('the active visit reconnects');
  second.dispatchEvent(new Event('open'));
  expect(second.sent).toEqual([first.sent[0]]);
  live.setPresence(null);
  stop();
  expect(second.sent.at(-1)).toEqual({
    type: 'setPresence',
    containerId: '',
    workspaceId: '',
    readToken: 'read-token',
  });
  expect(vi.getTimerCount()).toBe(0);
});
