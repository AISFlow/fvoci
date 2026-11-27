/**
 * The server's changes over the WebSocket (`lib/api/live.ts`), against a
 * stand-in socket: what the messages become, to whom they go, and the
 * reading again after the connection comes back.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { LiveChanges, type LiveReads } from '$lib/api/live.ts';
import { BackendError, type Change } from '$lib/state/index.ts';

class FakeSocket extends EventTarget {
  static made: FakeSocket[] = [];
  readonly sent: unknown[] = [];
  closed = false;
  readonly url: string;

  constructor(url: string) {
    super();
    this.url = url;
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

  drop(code = 1006): void {
    this.dispatchEvent(new CloseEvent('close', { code }));
  }
}

const BLOCK = {
  parentId: 'b1',
  createdAt: 1,
  updatedAt: 2,
  createdBy: 'u',
  modifiedBy: 'u',
  title: 'A card',
};

const reads: LiveReads = {
  members: (containerId) =>
    Promise.resolve([
      {
        containerId,
        userId: 'u',
        role: 'admin',
        synthetic: false,
      },
    ]),
};

/** Lets the promises that are due run. */
const settle = async (): Promise<void> => {
  for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
};

/** Hears every listening of a test begin: what each gave back ends it. */
const watchListenings = () => vi.spyOn(LiveChanges.prototype, 'watch');
let listenings: ReturnType<typeof watchListenings>;

beforeEach(() => {
  FakeSocket.made = [];
  vi.stubGlobal('WebSocket', FakeSocket);
  vi.useFakeTimers();
  listenings = watchListenings();
});

afterEach(() => {
  // The connections of a test are closed with it, by ending each of its listenings: left
  // open, they would hear the next test's browser going offline, and open sockets of their own.
  for (const listening of listenings.mock.results) {
    if (listening.type === 'return') listening.value();
  }
  vi.restoreAllMocks();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function connected(): Promise<{
  live: LiveChanges;
  board: Change[];
  self: Change[];
  other: Change[];
  socket: FakeSocket;
}> {
  const live = new LiveChanges('ws://here/ws', reads);
  live.enter('t');
  const board: Change[] = [];
  const self: Change[] = [];
  const other: Change[] = [];
  live.watch('b1', (change) => board.push(change));
  live.watch('b2', (change) => other.push(change));
  live.watch(undefined, (change) => self.push(change));
  await settle();
  const socket = FakeSocket.made[0];
  if (!socket) throw new Error('no socket');
  socket.open();
  return { live, board, self, other, socket };
}

describe('changes from the server', () => {
  it('opens one connection and subscribes to the workspace', async () => {
    const { socket } = await connected();
    expect(FakeSocket.made).toHaveLength(1);
    expect(socket.url).toBe('ws://here/ws');
    expect(socket.sent).toEqual([{ type: 'subscribeWorkspace', workspaceId: 't' }]);
  });

  it('hands a block to the listeners of its board alone, as the screens’ block', async () => {
    const { socket, board, other, self } = await connected();
    // A card's message carries the summary of its body in place of the body.
    const summary = {
      hasBody: true,
      tasksTotal: 3,
      tasksDone: 1,
      preview: { imageFileId: 'f1', text: 'To do' },
    };
    socket.message({
      type: 'blockChanged',
      workspaceId: 't',
      block: {
        ...BLOCK,
        id: 'c1',
        containerId: 'b1',
        type: 'card',
        fields: { properties: {} },
        body: null,
      },
      summary,
    });
    socket.message({
      type: 'blockChanged',
      workspaceId: 't',
      block: { ...BLOCK, id: 'c1', containerId: 'b1', deletedAt: 5, type: '', fields: {} },
    });
    await settle();
    expect(board).toMatchObject([
      {
        kind: 'blocks',
        containerId: 'b1',
        blocks: [{ id: 'c1', type: 'card', title: 'A card', ...summary }],
      },
      { kind: 'blocksDeleted', containerId: 'b1', blockIds: ['c1'] },
    ]);
    expect(other).toEqual([]);
    expect(self).toEqual([]);
  });

  it('hands a board to its listeners and to the reader’s own; a deleted one as deleted', async () => {
    const { socket, board, self } = await connected();
    const shape = {
      workspaceId: 't',
      channelId: '',
      createdBy: 'u',
      modifiedBy: 'u',
      visibility: 'private',
      minimumRole: '',
      title: 'B',
      description: '',
      icon: '',
      showDescription: false,
      isTemplate: false,
      templateVersion: 0,
      properties: {},
      cardProperties: [],
      createdAt: 1,
      updatedAt: 2,
    };
    socket.message({
      type: 'containerChanged',
      workspaceId: 't',
      container: { ...shape, id: 'b1' },
    });
    socket.message({
      type: 'containerChanged',
      workspaceId: 't',
      container: { ...shape, id: 'b1', deletedAt: 9 },
    });
    await settle();
    expect(board.map((change) => change.kind)).toEqual(['container', 'containerDeleted']);
    expect(self.map((change) => change.kind)).toEqual(['container', 'containerDeleted']);
  });

  it('reads all the members of a watched board when one changes, and the sidebar soon after', async () => {
    const members = vi.spyOn(reads, 'members');
    const { socket, board, self } = await connected();
    socket.message({
      type: 'containerMemberChanged',
      workspaceId: 't',
      member: { containerId: 'b1', userId: 'u2', role: 'viewer' },
    });
    // A board nobody here watches: its members are not read.
    socket.message({
      type: 'containerMemberRemoved',
      workspaceId: 't',
      member: { containerId: 'b9', userId: 'u2' },
    });
    await settle();
    expect(board).toMatchObject([
      { kind: 'members', containerId: 'b1', members: [{ userId: 'u' }] },
    ]);
    expect(members.mock.calls).toEqual([['b1']]);
    // Being added to a board, or taken off one, shows in the sidebar: read once for both.
    expect(self).toEqual([]);
    await vi.advanceTimersByTimeAsync(200);
    expect(self).toEqual([{ kind: 'resync' }]);
    members.mockRestore();
  });

  it('tells only the later of two readings of members, and a board taken away as gone', async () => {
    let answer: (value: Awaited<ReturnType<LiveReads['members']>>) => void = () => undefined;
    const members = vi
      .spyOn(reads, 'members')
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            answer = resolve;
          }),
      )
      .mockImplementationOnce(() => Promise.reject(new BackendError(403, { detail: 'no' })));
    const { socket, board } = await connected();
    const message = {
      type: 'containerMemberChanged',
      workspaceId: 't',
      member: { containerId: 'b1', userId: 'u2' },
    };
    socket.message(message);
    socket.message(message);
    await settle();
    answer([]);
    await settle();
    expect(board).toEqual([{ kind: 'containerDeleted', containerId: 'b1' }]);
    members.mockRestore();
  });

  it("tells the reader's own listeners whose name changed, and the board nothing", async () => {
    const { socket, self, board } = await connected();
    socket.message({ type: 'userChanged', workspaceId: 't', userId: 'u2' });
    // Of a workspace that was left: not told.
    socket.message({ type: 'userChanged', workspaceId: 'gone', userId: 'u3' });
    await settle();
    expect(self).toEqual([{ kind: 'user', userId: 'u2' }]);
    expect(board).toEqual([]);
  });

  it("tells the reader's own listeners that their notifications changed, whatever workspace is followed", async () => {
    const { socket, self, board } = await connected();
    socket.message({ type: 'notificationsChanged' });
    await settle();
    expect(self).toEqual([{ kind: 'notifications' }]);
    expect(board).toEqual([]);
  });

  it('reads the sidebar again on a message of the categories', async () => {
    const { socket, self, board } = await connected();
    socket.message({ type: 'categoryChanged', workspaceId: 't', category: {} });
    socket.message({ type: 'categoriesReordered', workspaceId: 't', categoryOrder: [] });
    await vi.advanceTimersByTimeAsync(200);
    expect(self).toEqual([{ kind: 'resync' }]);
    expect(board).toEqual([]);
  });

  it('asks again for a workspace whose subscription the server ended, and has everyone read again', async () => {
    const { socket, board, self } = await connected();
    socket.message({
      type: 'workspaceUnsubscribed',
      workspaceId: 't',
      reason: 'workspace access ended',
    });
    socket.message({
      type: 'workspaceUnsubscribed',
      workspaceId: 'left',
      reason: 'workspace access ended',
    });
    await settle();
    expect(socket.closed).toBe(false);
    expect(socket.sent).toEqual([
      { type: 'subscribeWorkspace', workspaceId: 't' },
      { type: 'subscribeWorkspace', workspaceId: 't' },
    ]);
    expect(board).toEqual([{ kind: 'resync' }]);
    expect(self).toEqual([{ kind: 'resync' }]);
  });

  it('asks again for its workspace whenever it is entered again, also on a connection made since the server ended it', async () => {
    const { live, socket, board } = await connected();
    const subscribe = { type: 'subscribeWorkspace', workspaceId: 't' };
    // As the server ends the own space for a session that has not shown the password:
    // asked again at once, which the server ignores and says nothing of.
    socket.message({ type: 'workspaceUnsubscribed', workspaceId: 't' });
    expect(socket.sent).toEqual([subscribe, subscribe]);
    expect(board).toEqual([{ kind: 'resync' }]);
    // The connection comes again meanwhile, and its asking is ignored as silently.
    live.restart();
    await settle();
    const again = FakeSocket.made[1];
    again?.open();
    expect(again?.sent).toEqual([subscribe]);
    // The session has shown the password, and the space is entered again: asked for
    // again, and whoever listens reads again what it did not hear while it was closed.
    board.length = 0;
    live.enter('t');
    expect(FakeSocket.made).toHaveLength(2);
    expect(again?.closed).toBe(false);
    expect(again?.sent).toEqual([subscribe, subscribe]);
    expect(board).toEqual([{ kind: 'resync' }]);
  });

  it('asks for nothing on entering again before its connection is open', () => {
    const live = new LiveChanges('ws://here/ws', reads);
    live.enter('t');
    live.enter('t');
    expect(FakeSocket.made).toHaveLength(0);
  });

  it('opens again at once after signing in, and has everyone read again', async () => {
    const { live, socket, board } = await connected();
    live.restart();
    expect(socket.closed).toBe(true);
    await settle();
    const again = FakeSocket.made[1];
    again?.open();
    expect(again?.sent).toEqual([{ type: 'subscribeWorkspace', workspaceId: 't' }]);
    expect(board).toEqual([{ kind: 'resync' }]);
  });

  it('opens nothing until a workspace is entered, and then subscribes to it', async () => {
    const live = new LiveChanges('ws://here/ws', reads);
    live.watch(undefined, () => undefined);
    await settle();
    expect(FakeSocket.made).toHaveLength(0);
    live.enter('t');
    FakeSocket.made[0]?.open();
    expect(FakeSocket.made[0]?.sent).toEqual([{ type: 'subscribeWorkspace', workspaceId: 't' }]);
  });

  it('moves the subscription to another workspace, and lets go of what the old one still sends', async () => {
    const { live, socket, self } = await connected();
    live.enter('t2');
    live.enter('t2');
    expect(FakeSocket.made).toHaveLength(1);
    expect(socket.sent).toEqual([
      { type: 'subscribeWorkspace', workspaceId: 't' },
      { type: 'unsubscribeWorkspace', workspaceId: 't' },
      { type: 'subscribeWorkspace', workspaceId: 't2' },
      // Entered again, asked for again: the server takes that for what is followed.
      { type: 'subscribeWorkspace', workspaceId: 't2' },
    ]);
    const board = (workspaceId: string, id: string): unknown => ({
      type: 'containerChanged',
      workspaceId,
      container: { id, workspaceId, title: id, cardProperties: [], properties: {} },
    });
    socket.message(board('t', 'old'));
    socket.message(board('t2', 'new'));
    await settle();
    // Entered again, the listeners read again; of the two boards only the new one's is told.
    expect(
      self.map((change) => (change.kind === 'container' ? change.container.id : change.kind)),
    ).toEqual(['resync', 'new']);
  });

  it('subscribes a connection that opens after entering to the workspace entered last', async () => {
    const live = new LiveChanges('ws://here/ws', reads);
    live.enter('t');
    live.watch('b1', () => undefined);
    live.enter('t2');
    FakeSocket.made[0]?.open();
    expect(FakeSocket.made).toHaveLength(1);
    expect(FakeSocket.made[0]?.sent).toEqual([{ type: 'subscribeWorkspace', workspaceId: 't2' }]);
  });

  it('opens again at once on signing in, subscribed to the workspace entered', async () => {
    const { live, socket } = await connected();
    live.restart();
    expect(socket.closed).toBe(true);
    FakeSocket.made[1]?.open();
    expect(FakeSocket.made[1]?.sent).toEqual([{ type: 'subscribeWorkspace', workspaceId: 't' }]);
  });

  it('waits from the first wait again only after a connection has held', async () => {
    const { socket } = await connected();
    socket.drop();
    await vi.advanceTimersByTimeAsync(500);
    // Opens, and is closed at once: the next wait is the longer one.
    FakeSocket.made[1]?.open();
    FakeSocket.made[1]?.drop();
    await vi.advanceTimersByTimeAsync(999);
    expect(FakeSocket.made).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(FakeSocket.made).toHaveLength(3);
    // This one holds; a drop after it waits the first wait.
    FakeSocket.made[2]?.open();
    await vi.advanceTimersByTimeAsync(10_000);
    FakeSocket.made[2]?.drop();
    await vi.advanceTimersByTimeAsync(500);
    expect(FakeSocket.made).toHaveLength(4);
  });

  it('opens again after a drop, waiting longer each time, and has everyone read again', async () => {
    const { socket, board, self, other } = await connected();
    socket.drop();
    await vi.advanceTimersByTimeAsync(499);
    expect(FakeSocket.made).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(FakeSocket.made).toHaveLength(2);
    FakeSocket.made[1]?.drop();
    await vi.advanceTimersByTimeAsync(999);
    expect(FakeSocket.made).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    const again = FakeSocket.made[2];
    again?.open();
    expect(again?.sent).toEqual([{ type: 'subscribeWorkspace', workspaceId: 't' }]);
    for (const changes of [board, self, other]) expect(changes).toEqual([{ kind: 'resync' }]);
  });

  it('drops the connection at once when the browser says it is offline, and opens again when it is back', async () => {
    const { socket, board } = await connected();
    globalThis.dispatchEvent(new Event('offline'));
    // Dropped without waiting for the socket to say so; its late close changes nothing.
    await vi.advanceTimersByTimeAsync(499);
    expect(FakeSocket.made).toHaveLength(1);
    socket.drop();
    await vi.advanceTimersByTimeAsync(1);
    expect(FakeSocket.made).toHaveLength(2);
    // Still offline: the try fails, and the next waits longer.
    FakeSocket.made[1]?.drop();
    await vi.advanceTimersByTimeAsync(500);
    expect(FakeSocket.made).toHaveLength(2);
    globalThis.dispatchEvent(new Event('online'));
    expect(FakeSocket.made).toHaveLength(3);
    FakeSocket.made[2]?.open();
    expect(board).toEqual([{ kind: 'resync' }]);
    // Offline with no connection open: nothing to drop.
    FakeSocket.made[2]?.drop();
    globalThis.dispatchEvent(new Event('offline'));
    await vi.advanceTimersByTimeAsync(2000);
    expect(FakeSocket.made).toHaveLength(4);
  });

  it('spreads the wait after the server asks to try again, so its pages do not all come back at once', async () => {
    const random = vi.spyOn(Math, 'random').mockReturnValue(0.5);
    try {
      const { socket } = await connected();
      socket.drop(1013);
      await vi.advanceTimersByTimeAsync(749);
      expect(FakeSocket.made).toHaveLength(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(FakeSocket.made).toHaveLength(2);
    } finally {
      random.mockRestore();
    }
  });

  it('closes the connection when nobody listens any more, and does not open it again', async () => {
    const live = new LiveChanges('ws://here/ws', reads);
    live.enter('t');
    const stop = live.watch('b1', () => undefined);
    await settle();
    const socket = FakeSocket.made[0];
    socket?.open();
    stop();
    expect(socket?.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(FakeSocket.made).toHaveLength(1);
  });

  it('triggers resync on workspaceSubscribed and blocksSubscribed ACKs', async () => {
    const { socket, board, self } = await connected();
    socket.message({ type: 'workspaceSubscribed', workspaceId: 't' });
    socket.message({ type: 'blocksSubscribed', workspaceId: 't', containerId: 'b1' });
    await settle();
    expect(board).toEqual([{ kind: 'resync' }, { kind: 'resync' }]);
    expect(self).toEqual([{ kind: 'resync' }, { kind: 'resync' }]);
  });

  it('forwards workspaceRoleChanged to listeners and schedules sidebar resync', async () => {
    const { socket, board, self } = await connected();
    socket.message({
      type: 'workspaceRoleChanged',
      workspaceId: 't',
      userId: 'u',
      role: 'admin',
    });
    await settle();
    expect(board).toEqual([
      { kind: 'workspaceRole', workspaceId: 't', userId: 'u', role: 'admin' },
    ]);
    expect(self).toEqual([{ kind: 'workspaceRole', workspaceId: 't', userId: 'u', role: 'admin' }]);
    await vi.advanceTimersByTimeAsync(200);
    expect(self).toEqual([
      { kind: 'workspaceRole', workspaceId: 't', userId: 'u', role: 'admin' },
      { kind: 'resync' },
    ]);
  });
});
