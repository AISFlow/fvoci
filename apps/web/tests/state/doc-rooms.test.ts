/**
 * A body opened in its room (`Backend.openDoc`): two readers of the backend
 * in memory edit one body together and the card's body follows, a reader
 * who may only read is let in to read, and the page's connection
 * (`api/live.ts` with `live-rooms.ts`) carries the room's messages and
 * frames to the room's client and back.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { LiveChanges } from '$lib/api/live.ts';
import {
  decodeCaretFrame,
  decodeDocFrame,
  encodeCaretFrame,
  encodeDocFrame,
  SYNC_STEP_1,
  SYNC_STEP_2,
  SYNC_UPDATE,
} from '$lib/logic/doc-frame.ts';
import type { BodyDoc, BodyDocHears, BodyDocState, Change } from '$lib/state/backend.ts';
import { CARET_WAIT } from '$lib/state/doc-carets.ts';
import { missingFrom, openRoom } from '$lib/state/doc-client.ts';
import { caretTold } from '$lib/state/memory/caret-frame.ts';
import { bodyOf, FIELD, roomsOf } from '$lib/state/memory/rooms.ts';

import { setup } from './support.ts';

/** What an opener of a body heard. */
function listener(): BodyDocHears & { states: BodyDocState[]; lost: string[] } {
  const states: BodyDocState[] = [];
  const lost: string[] = [];
  return {
    states,
    lost,
    state: (state) => states.push(state),
    discarded: (why) => lost.push(why),
  };
}

/** Writes a paragraph of `value` at the end of a body's document. */
function append(doc: Y.Doc, value: string): void {
  const paragraph = new Y.XmlElement('paragraph');
  paragraph.insert(0, [new Y.XmlText(value)]);
  const fragment = doc.getXmlFragment(FIELD);
  fragment.insert(fragment.length, [paragraph]);
}

const texts = (doc: Y.Doc | undefined): string[] =>
  doc
    ? doc
        .getXmlFragment(FIELD)
        .toArray()
        .map((node) => node.toJSON() as string)
    : [];

/** The world in memory, its first board, a card of it with a body, and two of its members. */
function world(): ReturnType<typeof setup> & { boardId: string; cardId: string } {
  const made = setup();
  const card = [...made.world.blocks.values()].find((block) => block.type === 'card' && block.body);
  if (!card) throw new Error('the seed has no card with a body');
  return { ...made, boardId: card.containerId, cardId: card.id };
}

async function joined(body: BodyDoc): Promise<Y.Doc> {
  await vi.waitFor(() => {
    expect(body.state.status).toBe('joined');
  });
  const { doc } = body.state;
  if (!doc) throw new Error('joined without a document');
  return doc;
}

describe('the rooms of the backend in memory', () => {
  it('lets two readers edit one body together, and writes the card from it', async () => {
    const { world: stored, backend, other, boardId, cardId } = world();
    const changes: Change[] = [];
    backend.watchContainer(boardId, (change) => changes.push(change));
    const mine = backend.openDoc('w', boardId, cardId, listener());
    const theirs = other.openDoc('w', boardId, cardId, listener());
    const [a, b] = await Promise.all([joined(mine), joined(theirs)]);
    // Both hold the server's document of the stored body, made once.
    expect(bodyOf(a)).toEqual(stored.blocks.get(cardId)?.body);
    expect(bodyOf(b)).toEqual(bodyOf(a));

    append(a, 'from a');
    append(b, 'from b');
    await vi.waitFor(() => {
      expect(texts(a)).toEqual(texts(b));
      expect(texts(a).slice(-2).sort()).toEqual([
        '<paragraph>from a</paragraph>',
        '<paragraph>from b</paragraph>',
      ]);
      expect(mine.state.pending || theirs.state.pending).toBe(false);
    });

    roomsOf(stored).flush();
    const card = stored.blocks.get(cardId);
    expect(card?.body).toEqual(bodyOf(a));
    await vi.waitFor(() => {
      expect(changes.some((change) => change.kind === 'blocks')).toBe(true);
    });
    mine.close();
    theirs.close();
    await vi.waitFor(() => {
      expect(roomsOf(stored).members(cardId)).toBe(0);
    });
  });

  it('takes what a reader sent in the order sent, also what arrives at one moment', async () => {
    const { backend, boardId, cardId } = world();
    const told = listener();
    const mine = backend.openDoc('w', boardId, cardId, told);
    const a = await joined(mine);
    // Slow, then fast: what is sent next arrives at the moment the slow one does, and each
    // change rests on the one before it. (Here the timers of one moment fire in the order
    // they were made; a browser's need not, which the card's browser tests would show.)
    backend.latency = 300;
    append(a, 'slow');
    await new Promise((resolve) => setTimeout(resolve, 40));
    backend.latency = 0;
    for (let sent = 0; sent < 12; sent += 1) {
      append(a, `fast ${String(sent)}`);
      await new Promise((resolve) => setTimeout(resolve, 35));
    }
    await vi.waitFor(() => {
      expect(mine.state.pending).toBe(false);
    });
    expect(told.lost).toEqual([]);
    expect(mine.state.status).toBe('joined');
    expect(texts(mine.state.doc).filter((line) => line.includes('fast'))).toHaveLength(12);
    mine.close();
  });

  it('sends who comes in again with the document of this making its state vector and what they lack', async () => {
    const { world: stored, backend, other, boardId, cardId } = world();
    const frames: { sync: number; payload: Uint8Array }[] = [];
    const joins: number[] = [];
    const port = roomsOf(stored).line(
      backend,
      boardId,
      cardId,
    )({
      up: () => undefined,
      down: () => undefined,
      message: (message) => {
        if (message.type === 'docJoined') joins.push(message.epoch);
      },
      frame: (frame) => frames.push(frame),
      carets: () => undefined,
    });
    port.join();
    await vi.waitFor(() => {
      expect(frames).toHaveLength(1);
    });
    const held = new Y.Doc();
    Y.applyUpdate(held, frames[0]?.payload ?? new Uint8Array());
    // The words of the body as it was stored: the whole carries them, a difference need not.
    const storedWords = held.getXmlFragment(FIELD).toDOM().textContent ?? '';
    const carries = (payload: Uint8Array | undefined): boolean =>
      new TextDecoder().decode(payload).includes(storedWords);
    expect(storedWords).not.toBe('');
    expect(carries(frames[0]?.payload)).toBe(true);
    // Somebody else writes while this reader is away with the document.
    const theirs = backend.openDoc('w', boardId, cardId, listener());
    append(await joined(theirs), 'meanwhile');
    await vi.waitFor(() => {
      expect(theirs.state.pending).toBe(false);
    });

    // (In the room, the seat was sent that change too; the document it went away with was not.)
    await vi.waitFor(() => {
      expect(frames).toHaveLength(2);
    });
    port.join({ epoch: joins[0] ?? 0, stateVector: Y.encodeStateVector(held) });
    await vi.waitFor(() => {
      expect(frames).toHaveLength(4);
    });
    expect(frames.slice(2).map((frame) => frame.sync)).toEqual([SYNC_STEP_1, SYNC_STEP_2]);
    expect(carries(frames[3]?.payload)).toBe(false);
    Y.applyUpdate(held, frames[3]?.payload ?? new Uint8Array());
    expect(texts(held)).toEqual(texts(theirs.state.doc));
    // Who only reads is sent the whole, whatever they name.
    await backend.setMemberRole(boardId, other.userId, 'viewer');
    const read: { sync: number; payload: Uint8Array }[] = [];
    const reader = roomsOf(stored).line(
      other,
      boardId,
      cardId,
    )({
      up: () => undefined,
      down: () => undefined,
      message: () => undefined,
      frame: (frame) => read.push(frame),
      carets: () => undefined,
    });
    reader.join({ epoch: joins[0] ?? 0, stateVector: Y.encodeStateVector(held) });
    await vi.waitFor(() => {
      expect(read).toHaveLength(1);
    });
    expect([read[0]?.sync, carries(read[0]?.payload)]).toEqual([SYNC_STEP_2, true]);
    reader.close();
    // Of another epoch, or with what is no state vector: the whole.
    port.join({ epoch: 99, stateVector: Y.encodeStateVector(held) });
    port.join({ epoch: joins[0] ?? 0, stateVector: new Uint8Array([5]) });
    await vi.waitFor(() => {
      expect(frames).toHaveLength(6);
    });
    expect(frames.slice(4).map((frame) => frame.sync)).toEqual([SYNC_STEP_2, SYNC_STEP_2]);
    expect(frames.slice(4).map((frame) => carries(frame.payload))).toEqual([true, true]);
    port.close();
    theirs.close();
  });

  it('lets a viewer in to read: they see the changes and cannot write', async () => {
    const { world: stored, backend, other, boardId, cardId } = world();
    await backend.setMemberRole(boardId, other.userId, 'viewer');
    const mine = backend.openDoc('w', boardId, cardId, listener());
    const told = listener();
    const theirs = other.openDoc('w', boardId, cardId, told);
    const [a, b] = await Promise.all([joined(mine), joined(theirs)]);
    expect(theirs.state).toMatchObject({ mode: 'read', writable: false });
    append(a, 'from a');
    await vi.waitFor(() => {
      expect(texts(b)).toEqual(texts(a));
    });
    // Written in a document read only (as an editor's own repair would): never sent.
    append(b, 'from b');
    const length = texts(a).length;
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(texts(a)).toHaveLength(length);
    roomsOf(stored).flush();
    expect(stored.blocks.get(cardId)?.body).toEqual(bodyOf(a));
    mine.close();
    theirs.close();
  });

  it('turns a reader whose role is lowered to reading, and puts out one taken off the board', async () => {
    const { backend, other, boardId, cardId } = world();
    const theirs = other.openDoc('w', boardId, cardId, listener());
    await joined(theirs);
    await backend.setMemberRole(boardId, other.userId, 'commenter');
    await vi.waitFor(() => {
      expect(theirs.state).toMatchObject({ mode: 'read', writable: false });
    });
    await backend.removeMember(boardId, other.userId);
    await vi.waitFor(() => {
      expect(theirs.state).toMatchObject({ status: 'out', reason: 'not_found', doc: undefined });
    });
    theirs.close();
  });

  it('refuses who is no member', async () => {
    const { backend, other, boardId, cardId } = world();
    await backend.removeMember(boardId, other.userId);
    const theirs = other.openDoc('w', boardId, cardId, listener());
    await vi.waitFor(() => {
      expect(theirs.state).toMatchObject({ status: 'out', reason: 'not_found', retrying: false });
    });
  });
});

class FakeSocket extends EventTarget {
  static made: FakeSocket[] = [];
  readonly texts: Record<string, unknown>[] = [];
  readonly frames: Uint8Array[] = [];
  binaryType = 'blob';
  readonly url: string;

  constructor(url: string) {
    super();
    this.url = url;
    FakeSocket.made.push(this);
  }

  send(data: string | Uint8Array): void {
    if (typeof data === 'string') this.texts.push(JSON.parse(data) as Record<string, unknown>);
    else this.frames.push(data);
  }

  close(): void {
    // The page closed it: no event, as the live connection no longer listens.
  }

  open(): void {
    this.dispatchEvent(new Event('open'));
  }

  message(data: unknown): void {
    this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(data) }));
  }

  binary(bytes: Uint8Array): void {
    const buffer = bytes.slice().buffer;
    this.dispatchEvent(new MessageEvent('message', { data: buffer }));
  }

  drop(): void {
    this.dispatchEvent(new Event('close'));
  }
}

describe('a room on the page connection', () => {
  beforeEach(() => {
    FakeSocket.made = [];
    vi.stubGlobal('WebSocket', FakeSocket);
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  function server(): { live: LiveChanges; socket: () => FakeSocket } {
    const live = new LiveChanges('ws://test/ws', { members: () => Promise.resolve([]) });
    live.enter('w1');
    return {
      live,
      socket: () => {
        const socket = FakeSocket.made.at(-1);
        if (!socket) throw new Error('no socket');
        return socket;
      },
    };
  }

  /** The server's document of a body with one paragraph. */
  function whole(): Uint8Array {
    const doc = new Y.Doc();
    append(doc, 'stored');
    return Y.encodeStateAsUpdate(doc);
  }

  it('opens the connection for a room, joins, takes the whole, and sends a change as a frame', () => {
    const { live, socket } = server();
    const told = listener();
    const body = openRoom(live.rooms.line('w1', 'c1'), told);
    socket().open();
    expect(socket().texts).toContainEqual({ type: 'joinDoc', workspaceId: 'w1', cardId: 'c1' });
    socket().message({
      type: 'docJoined',
      workspaceId: 'w1',
      cardId: 'c1',
      doc: 5,
      epoch: 2,
      mode: 'write',
    });
    // A message of the subscription comes between the join and the whole: no harm.
    socket().message({ type: 'containerChanged', workspaceId: 'w1', container: { id: 'b' } });
    socket().binary(encodeDocFrame({ doc: 5, epoch: 2, sync: SYNC_STEP_2, payload: whole() }));
    expect(body.state.status).toBe('joined');
    const doc = body.state.doc;
    if (!doc) throw new Error('no document');
    expect(texts(doc)).toEqual(['<paragraph>stored</paragraph>']);

    append(doc, 'mine');
    expect(socket().frames).toHaveLength(0);
    vi.advanceTimersByTime(50);
    expect(socket().frames).toHaveLength(1);
    const sent = decodeDocFrame(socket().frames[0] ?? new Uint8Array());
    expect(sent).toMatchObject({ doc: 5, epoch: 2, sync: SYNC_UPDATE });
    expect(body.state.pending).toBe(true);
    socket().message({ type: 'docAck', doc: 5, epoch: 2, seq: 1 });
    expect(body.state.pending).toBe(false);

    // Another's change; and one of a room not this one, dropped.
    const theirs = new Y.Doc();
    Y.applyUpdate(theirs, Y.encodeStateAsUpdate(doc));
    const before = Y.encodeStateVector(theirs);
    append(theirs, 'theirs');
    const change = Y.encodeStateAsUpdate(theirs, before);
    socket().binary(encodeDocFrame({ doc: 9, epoch: 2, sync: SYNC_UPDATE, payload: change }));
    expect(texts(doc)).toHaveLength(2);
    socket().binary(encodeDocFrame({ doc: 5, epoch: 2, sync: SYNC_UPDATE, payload: change }));
    expect(texts(doc)).toHaveLength(3);

    body.close();
    expect(socket().texts.at(-1)).toEqual({ type: 'leaveDoc', doc: 5 });
  });

  it('is read only while the connection is away, and joins again when it is back', () => {
    const { live, socket } = server();
    const stored = whole();
    const body = openRoom(live.rooms.line('w1', 'c1'), listener());
    socket().open();
    socket().message({
      type: 'docJoined',
      workspaceId: 'w1',
      cardId: 'c1',
      doc: 1,
      epoch: 2,
      mode: 'write',
    });
    socket().binary(encodeDocFrame({ doc: 1, epoch: 2, sync: SYNC_STEP_2, payload: stored }));
    const first = socket();
    first.drop();
    expect(body.state).toMatchObject({ status: 'offline', writable: false });
    vi.advanceTimersByTime(600);
    expect(socket()).not.toBe(first);
    socket().open();
    // The join names the document held: its epoch, and its state vector in base64.
    const held = body.state.doc;
    if (!held) throw new Error('no document');
    const stateVector = Buffer.from(Y.encodeStateVector(held)).toString('base64');
    expect(socket().texts).toContainEqual({
      type: 'joinDoc',
      workspaceId: 'w1',
      cardId: 'c1',
      epoch: 2,
      stateVector,
    });
    expect(body.state.status).toBe('receiving');
    socket().message({
      type: 'docJoined',
      workspaceId: 'w1',
      cardId: 'c1',
      doc: 1,
      epoch: 2,
      mode: 'write',
    });
    socket().binary(encodeDocFrame({ doc: 1, epoch: 2, sync: SYNC_STEP_2, payload: stored }));
    expect(body.state).toMatchObject({ status: 'joined', writable: true });
    // The same epoch: the document held goes on, nothing doubled.
    expect(texts(body.state.doc)).toHaveLength(1);
    body.close();
  });

  it('back after a drop, takes what it lacks, sends what the server lacks, and tells its caret again', () => {
    const { live, socket } = server();
    const told = listener();
    const body = openRoom(live.rooms.line('w1', 'c1'), told);
    const joined = (doc: number): void => {
      socket().message({
        type: 'docJoined',
        workspaceId: 'w1',
        cardId: 'c1',
        doc,
        epoch: 2,
        mode: 'write',
      });
    };
    const theirs = new Y.Doc();
    Y.applyUpdate(theirs, whole());
    socket().open();
    joined(1);
    socket().binary(
      encodeDocFrame({
        doc: 1,
        epoch: 2,
        sync: SYNC_STEP_2,
        payload: Y.encodeStateAsUpdate(theirs),
      }),
    );
    const { doc, carets } = body.state;
    if (!doc || !carets) throw new Error('no document');
    // The reader is in the body, and somebody else is too.
    const place = Y.createRelativePositionFromTypeIndex(doc.getXmlFragment(FIELD), 0);
    carets.setLocalStateField('cursor', { anchor: place, head: place });
    vi.advanceTimersByTime(CARET_WAIT);
    const caretFrames = (of: FakeSocket): number =>
      of.frames.filter((frame) => decodeCaretFrame(frame) !== undefined).length;
    expect(caretFrames(socket())).toBe(1);
    const other = '{"user":{"id":"u2"},"cursor":null}';
    socket().binary(encodeCaretFrame({ doc: 1, epoch: 2, update: caretTold(77, 1, other) }));
    expect(carets.getStates().has(77)).toBe(true);

    // The connection drops; both write meanwhile, the reader deleting what both held.
    socket().drop();
    expect(carets.getStates().has(77)).toBe(false);
    append(theirs, 'theirs meanwhile');
    doc.getXmlFragment(FIELD).delete(0, 1);
    append(doc, 'mine meanwhile');
    vi.advanceTimersByTime(600);
    socket().open();
    joined(2);
    // The server's state vector, and what the reader lacks of its document: not the whole.
    const lacked = Y.encodeStateAsUpdate(theirs, Y.encodeStateVector(doc));
    socket().binary(
      encodeDocFrame({ doc: 2, epoch: 2, sync: SYNC_STEP_1, payload: Y.encodeStateVector(theirs) }),
    );
    socket().binary(encodeDocFrame({ doc: 2, epoch: 2, sync: SYNC_STEP_2, payload: lacked }));
    expect(body.state).toMatchObject({ status: 'joined', writable: true, pending: true });
    expect(texts(doc)).toContain('<paragraph>theirs meanwhile</paragraph>');
    // What the server lacks goes as one change; taken, both hold the same.
    vi.advanceTimersByTime(50);
    const sent = socket()
      .frames.map((frame) => decodeDocFrame(frame))
      .filter((frame) => frame !== undefined);
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({ doc: 2, epoch: 2, sync: SYNC_UPDATE });
    Y.applyUpdate(theirs, sent[0]?.payload ?? new Uint8Array());
    socket().message({ type: 'docAck', doc: 2, epoch: 2, seq: 9 });
    expect(body.state.pending).toBe(false);
    expect(texts(theirs)).toEqual(texts(doc));
    // What both held was deleted by the reader; what each wrote meanwhile is there.
    expect([...texts(doc)].sort()).toEqual([
      '<paragraph>mine meanwhile</paragraph>',
      '<paragraph>theirs meanwhile</paragraph>',
    ]);
    expect(told.lost).toEqual([]);
    // The room is told where the reader is again, in the room's new number.
    vi.advanceTimersByTime(CARET_WAIT);
    const again = socket()
      .frames.map((frame) => decodeCaretFrame(frame))
      .filter((frame) => frame !== undefined);
    expect(again).toHaveLength(1);
    expect(again[0]).toMatchObject({ doc: 2, epoch: 2 });
    body.close();
  });

  it('sends all it holds when the state vector of the server cannot be read', () => {
    const { live, socket } = server();
    const body = openRoom(live.rooms.line('w1', 'c1'), listener());
    const joined = (doc: number): void => {
      socket().message({
        type: 'docJoined',
        workspaceId: 'w1',
        cardId: 'c1',
        doc,
        epoch: 2,
        mode: 'write',
      });
    };
    socket().open();
    joined(1);
    socket().binary(encodeDocFrame({ doc: 1, epoch: 2, sync: SYNC_STEP_2, payload: whole() }));
    const doc = body.state.doc;
    if (!doc) throw new Error('no document');
    socket().drop();
    append(doc, 'mine meanwhile');
    vi.advanceTimersByTime(600);
    socket().open();
    joined(2);
    // Five clients, the state vector says, and names none.
    const unreadable = new Uint8Array([5]);
    socket().binary(encodeDocFrame({ doc: 2, epoch: 2, sync: SYNC_STEP_1, payload: unreadable }));
    socket().binary(
      encodeDocFrame({ doc: 2, epoch: 2, sync: SYNC_STEP_2, payload: new Uint8Array([0, 0]) }),
    );
    vi.advanceTimersByTime(50);
    const sent = socket()
      .frames.map((frame) => decodeDocFrame(frame))
      .filter((frame) => frame !== undefined);
    expect(sent).toHaveLength(1);
    const theirs = new Y.Doc();
    Y.applyUpdate(theirs, sent[0]?.payload ?? new Uint8Array());
    expect(texts(theirs)).toEqual(texts(doc));
    // Not taken as known to the server until it says so.
    expect(body.state.pending).toBe(true);
    body.close();
  });

  it('gives each of two cards its answer, in the order they were asked', () => {
    const { live, socket } = server();
    const one = openRoom(live.rooms.line('w1', 'c1'), listener());
    const two = openRoom(live.rooms.line('w1', 'c2'), listener());
    socket().open();
    socket().message({
      type: 'docRefused',
      workspaceId: 'w1',
      cardId: 'c2',
      reason: 'not_found',
    });
    socket().message({
      type: 'docJoined',
      workspaceId: 'w1',
      cardId: 'c1',
      doc: 3,
      epoch: 1,
      mode: 'read',
    });
    expect(two.state).toMatchObject({ status: 'out', reason: 'not_found' });
    expect(one.state.status).toBe('joining');
    one.close();
    two.close();
  });

  it('refuses a second opening of a card on the page while the first is open, and lets it in after', async () => {
    const { live, socket } = server();
    const first = openRoom(live.rooms.line('w1', 'c1'), listener());
    socket().open();
    const second = openRoom(live.rooms.line('w1', 'c1'), listener());
    await Promise.resolve();
    expect(second.state).toMatchObject({ status: 'out', reason: 'already_open', retrying: true });
    expect(socket().texts.filter((text) => text.type === 'joinDoc')).toHaveLength(1);
    first.close();
    vi.advanceTimersByTime(1000);
    expect(socket().texts.filter((text) => text.type === 'joinDoc')).toHaveLength(2);
    second.close();
  });

  it('leaves the room of a card closed before its answer when the join after it is refused', () => {
    const { live, socket } = server();
    // A listener keeps the connection while no room is open.
    live.watch(undefined, () => undefined);
    socket().open();
    const first = openRoom(live.rooms.line('w1', 'c1'), listener());
    first.close();
    const second = openRoom(live.rooms.line('w1', 'c1'), listener());
    socket().message({
      type: 'docJoined',
      workspaceId: 'w1',
      cardId: 'c1',
      doc: 4,
      epoch: 1,
      mode: 'write',
    });
    expect(socket().texts.some((text) => text.type === 'leaveDoc')).toBe(false);
    socket().message({
      type: 'docRefused',
      workspaceId: 'w1',
      cardId: 'c1',
      reason: 'too_many_frames',
    });
    expect(socket().texts.at(-1)).toEqual({ type: 'leaveDoc', doc: 4 });
    second.close();
  });

  it('closes the connection once neither a room nor a listener needs it', () => {
    const { live, socket } = server();
    const body = openRoom(live.rooms.line('w1', 'c1'), listener());
    socket().open();
    const closing = vi.spyOn(socket(), 'close');
    body.close();
    expect(closing).toHaveBeenCalled();
  });
});

describe('what a document held has that the server lacks', () => {
  it('is nothing when it has nothing more, and a deletion made here alone is something', () => {
    const server = new Y.Doc();
    append(server, 'one');
    append(server, 'two');
    const whole = Y.encodeStateAsUpdate(server);
    const held = new Y.Doc();
    Y.applyUpdate(held, whole);
    expect(missingFrom(held, whole)).toBeUndefined();
    held.getXmlFragment(FIELD).delete(0, 1);
    const missing = missingFrom(held, whole);
    expect(missing).toBeDefined();
    Y.applyUpdate(server, missing ?? new Uint8Array());
    expect(texts(server)).toEqual(texts(held));
  });
});
