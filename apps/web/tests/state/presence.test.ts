import { afterEach, describe, expect, it, vi } from 'vitest';

import type { PresenceSnapshot } from '$lib/state/presence.ts';
import { LivePresence } from '$lib/api/live-presence.ts';
import { createBoardView } from '$lib/logic/wire.ts';
import { setup } from './support.ts';

afterEach(() => {
  vi.useRealTimers();
});

describe('presence in memory', () => {
  it('two instances see visits and open cards, deduplicate tabs, and remove a departure', async () => {
    vi.useFakeTimers();
    const { backend, other, world } = setup();
    const board = [...world.containers.values()][0];
    if (!board) throw new Error('a seeded board');
    const card = [...world.blocks.values()].find((block) => block.type === 'card');
    if (!card) throw new Error('a seeded card');
    await backend.addMember(board.id, other.userId, 'viewer');
    const place = { workspaceId: board.workspaceId, containerId: board.id };
    const heard: PresenceSnapshot[] = [];
    const stop = backend.presence.watchPresence(board.id, (snapshot) => heard.push(snapshot));
    backend.presence.setPresence(place);
    other.presence.setPresence({ ...place, cardId: card.id });
    await vi.advanceTimersByTimeAsync(250);
    expect(heard.at(-1)?.people).toHaveLength(2);
    expect(heard.at(-1)?.people.find((p) => p.userId === other.userId)?.cardIds).toEqual([card.id]);
    const anotherTab = other.as(other.userId);
    anotherTab.presence.setPresence({ ...place, cardId: card.id });
    await vi.advanceTimersByTimeAsync(500);
    expect(heard).toHaveLength(1);
    other.presence.setPresence(null);
    await vi.advanceTimersByTimeAsync(250);
    expect(heard).toHaveLength(1);
    anotherTab.presence.setPresence(null);
    await vi.advanceTimersByTimeAsync(250);
    expect(heard.at(-1)?.people).toEqual([{ userId: backend.userId, cardIds: [] }]);
    backend.presence.setPresence(null);
    stop();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('a wiki is visited as a board is, and its page as a card', async () => {
    vi.useFakeTimers();
    const { backend, other, world } = setup();
    const wiki = await backend.wiki.createWiki(world.workspace.id, {
      title: 'Handbook',
      visibility: 'private',
    });
    const page = await backend.wiki.createPage(wiki.id, { title: 'Welcome' });
    await backend.addMember(wiki.id, other.userId, 'editor');
    const place = { workspaceId: wiki.workspaceId, containerId: wiki.id };
    const heard: PresenceSnapshot[] = [];
    const stop = backend.presence.watchPresence(wiki.id, (snapshot) => heard.push(snapshot));
    backend.presence.setPresence(place);
    other.presence.setPresence({ ...place, cardId: page.id });
    await vi.advanceTimersByTimeAsync(250);
    expect(heard.at(-1)?.people.find((p) => p.userId === other.userId)?.cardIds).toEqual([page.id]);
    other.presence.setPresence(null);
    backend.presence.setPresence(null);
    await vi.advanceTimersByTimeAsync(250);
    stop();
  });

  it('an uninvited user cannot enter and a removed member disappears', async () => {
    vi.useFakeTimers();
    const { backend, other, world } = setup();
    const { board } = await backend.createBoard(
      world.workspace.id,
      { title: 'Private', visibility: 'private' },
      { title: 'Board', fields: createBoardView().fields },
    );
    const place = { workspaceId: board.workspaceId, containerId: board.id };
    const heard: PresenceSnapshot[] = [];
    const stop = backend.presence.watchPresence(board.id, (snapshot) => heard.push(snapshot));
    backend.presence.setPresence(place);
    other.presence.setPresence(place);
    await vi.advanceTimersByTimeAsync(250);
    expect(heard.at(-1)?.people).toHaveLength(1);
    await backend.addMember(board.id, other.userId, 'viewer');
    other.presence.setPresence(place);
    await vi.advanceTimersByTimeAsync(250);
    expect(heard.at(-1)?.people).toHaveLength(2);
    await backend.removeMember(board.id, other.userId);
    await vi.advanceTimersByTimeAsync(250);
    expect(heard.at(-1)?.people).toHaveLength(1);
    other.presence.setPresence(null);
    backend.presence.setPresence(null);
    stop();
  });
});

it('the live connection renews after reconnect, clears stale rosters and stops its heartbeat', () => {
  vi.useFakeTimers();
  const send = vi.fn();
  const live = new LivePresence(send, vi.fn());
  const hear = vi.fn();
  const stop = live.watchPresence('board', hear);
  live.setPresence({ workspaceId: 'workspace', containerId: 'board', cardId: 'card' });
  expect(send).not.toHaveBeenCalled();
  live.up();
  expect(send).toHaveBeenLastCalledWith({
    type: 'setPresence',
    workspaceId: 'workspace',
    containerId: 'board',
    cardId: 'card',
  });
  live.text({
    type: 'presenceChanged',
    containerId: 'board',
    people: [{ userId: 'other', cardIds: [] }],
    anonymousCount: 0,
    hiddenCount: 0,
  });
  expect(hear).toHaveBeenCalledTimes(1);
  live.down();
  expect(hear).toHaveBeenLastCalledWith({ people: [], anonymousCount: 0, hiddenCount: 0 });
  live.up();
  expect(send).toHaveBeenCalledTimes(2);
  live.setPresence(null);
  stop();
  live.down();
  expect(live.wanted).toBe(false);
  expect(vi.getTimerCount()).toBe(0);
});

it('separate worlds exchange visits through BroadcastChannel and close the channel on departure', async () => {
  const first = setup();
  const second = setup();
  const channel = `presence-test-${crypto.randomUUID()}`;
  first.world.shareRooms = channel;
  second.world.shareRooms = channel;
  const board = [...first.world.containers.values()][0];
  if (!board) throw new Error('a seeded board');
  await first.backend.addMember(board.id, second.other.userId, 'viewer');
  await second.backend.addMember(board.id, second.other.userId, 'viewer');
  const place = { workspaceId: board.workspaceId, containerId: board.id };
  let latest: PresenceSnapshot | undefined;
  const stop = first.backend.presence.watchPresence(board.id, (snapshot) => {
    latest = snapshot;
  });
  try {
    first.backend.presence.setPresence(place);
    second.other.presence.setPresence(place);
    await vi.waitFor(() => {
      expect(latest?.people).toHaveLength(2);
    });
    second.other.presence.setPresence(null);
    await vi.waitFor(() => {
      expect(latest?.people).toHaveLength(1);
    });
  } finally {
    first.backend.presence.setPresence(null);
    second.other.presence.setPresence(null);
    stop();
  }
});

it('a hidden page leaves after one minute, returns when visible, and releases its visit on unmount', async () => {
  const { openFirstBoard } = await import('./support.ts');
  const { board, backend } = await openFirstBoard();
  vi.useFakeTimers();
  let hidden = false;
  const hiddenRead = vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden);
  const send = vi.spyOn(backend.presence, 'setPresence');
  const { boardPresence } = await import('$lib/state/board-presence.svelte.ts');
  const stop = boardPresence(board).start();
  try {
    expect(send).toHaveBeenLastCalledWith(
      expect.objectContaining({ containerId: board.containerId }),
    );
    hidden = true;
    document.dispatchEvent(new Event('visibilitychange'));
    await vi.advanceTimersByTimeAsync(59_999);
    expect(send).not.toHaveBeenLastCalledWith(null);
    await vi.advanceTimersByTimeAsync(1);
    expect(send).toHaveBeenLastCalledWith(null);
    hidden = false;
    document.dispatchEvent(new Event('visibilitychange'));
    expect(send).toHaveBeenLastCalledWith(
      expect.objectContaining({ containerId: board.containerId }),
    );
    window.dispatchEvent(new Event('pagehide'));
    expect(send).toHaveBeenLastCalledWith(null);
    window.dispatchEvent(new Event('pageshow'));
    expect(send).toHaveBeenLastCalledWith(
      expect.objectContaining({ containerId: board.containerId }),
    );
  } finally {
    stop();
    hiddenRead.mockRestore();
  }
  expect(send).toHaveBeenLastCalledWith(null);
  expect(vi.getTimerCount()).toBe(0);
});

it('the live connection asks once for the same visit, and leaving forgets the roster heard', () => {
  const send = vi.fn();
  const live = new LivePresence(send, vi.fn());
  const hear = vi.fn();
  const stop = live.watchPresence('board', hear);
  live.up();
  const place = { workspaceId: 'workspace', containerId: 'board' };
  live.setPresence(place);
  live.setPresence({ ...place });
  live.setPresence({ ...place, cardId: '' });
  expect(send).toHaveBeenCalledTimes(1);
  live.setPresence({ ...place, cardId: 'card' });
  expect(send).toHaveBeenCalledTimes(2);
  live.text({
    type: 'presenceChanged',
    containerId: 'board',
    people: [{ userId: 'other', cardIds: ['card'] }],
    anonymousCount: 1,
    hiddenCount: 0,
  });
  live.setPresence(null);
  expect(send).toHaveBeenLastCalledWith({ type: 'setPresence', containerId: '' });
  expect(hear).toHaveBeenLastCalledWith({ people: [], anonymousCount: 0, hiddenCount: 0 });
  stop();
  live.down();
});
