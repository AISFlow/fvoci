/**
 * The carets of a body opened in its room, between readers of the backend
 * in memory (`memory/room-carets.ts` through the room's client): each sees
 * where the others who write are, under the room's word of whose a caret
 * is; who reads shows none; a caret goes with its reader; and the tabs of a
 * browser tell each other's.
 */
import type { Awareness } from 'y-protocols/awareness';
import { describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import type { BodyDoc, BodyDocHears } from '$lib/state/backend.ts';
import { HELD_IDLE } from '$lib/state/doc-client.ts';
import { tabKey } from '$lib/state/memory/room-docs.ts';
import { FIELD } from '$lib/state/memory/rooms.ts';
import type { StoredCard } from '$lib/state/memory/world.ts';

import { setup } from './support.ts';

const hears = (): BodyDocHears => ({ state: () => undefined, discarded: () => undefined });

/** What makes the rooms of two tabs one, for a card of a world as it was seeded (`tabKey`). */
function tabOf(stored: ReturnType<typeof setup>['world'], cardId: string): string {
  const card = stored.blocks.get(cardId);
  if (card?.type !== 'card') throw new Error('no such card');
  return tabKey(card as StoredCard, cardId, 1);
}

function world(): ReturnType<typeof setup> & { boardId: string; cardId: string } {
  const made = setup();
  const card = [...made.world.blocks.values()].find((block) => block.type === 'card' && block.body);
  if (!card) throw new Error('the seed has no card with a body');
  return { ...made, boardId: card.containerId, cardId: card.id };
}

/** The carets of a body once it is joined. */
async function caretsOf(body: BodyDoc): Promise<{ awareness: Awareness; doc: Y.Doc }> {
  await vi.waitFor(() => {
    expect(body.state.status).toBe('joined');
  });
  const { carets, doc } = body.state;
  if (!carets || !doc) throw new Error('joined without a document and its carets');
  return { awareness: carets, doc };
}

/** Puts the reader's caret at the start of the body, as the editor's plugin does; the place as the room tells it. */
function show(held: { awareness: Awareness; doc: Y.Doc }, user: object = {}): object {
  const place = Y.createRelativePositionFromTypeIndex(held.doc.getXmlFragment(FIELD), 0);
  held.awareness.setLocalStateField('user', user);
  held.awareness.setLocalStateField('cursor', { anchor: place, head: place });
  // As the room tells it: what the place says, without what is null.
  const said = JSON.parse(JSON.stringify(place)) as Record<string, unknown>;
  return Object.fromEntries(Object.entries(said).filter(([, value]) => value !== null));
}

/** The states of the others a reader holds: all but their own. */
const others = (awareness: Awareness): Record<string, unknown>[] =>
  [...awareness.getStates()]
    .filter(([client]) => client !== awareness.clientID)
    .map(([, state]) => state);

describe('the carets of the rooms in memory', () => {
  it("shows each who writes where the other is, as the room's word of whose the caret is", async () => {
    const { backend, other, boardId, cardId } = world();
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const theirs = other.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([caretsOf(mine), caretsOf(theirs)]);

    // As somebody else: nothing of it but the place is told.
    const place = show(a, { id: other.userId, name: 'Mallory', color: '#ff0000' });
    await vi.waitFor(() => {
      expect(others(b.awareness)).toEqual([
        { user: { id: backend.userId }, cursor: { anchor: place, head: place } },
      ]);
    });
    // Nobody is told their own caret.
    expect(others(a.awareness)).toEqual([]);
    show(b);
    await vi.waitFor(() => {
      expect(others(a.awareness)).toMatchObject([{ user: { id: other.userId } }]);
    });

    // The caret withdrawn (the editor lost the focus) is gone for the other.
    a.awareness.setLocalStateField('cursor', null);
    await vi.waitFor(() => {
      expect(others(b.awareness)).toEqual([]);
    });
    mine.close();
    theirs.close();
  });

  it('tells a reader the carets and shows none of theirs; a writer made a reader loses theirs', async () => {
    const { backend, other, boardId, cardId } = world();
    await backend.setMemberRole(boardId, other.userId, 'viewer');
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const theirs = other.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([caretsOf(mine), caretsOf(theirs)]);
    expect(theirs.state.mode).toBe('read');
    show(a);
    show(b);
    await vi.waitFor(() => {
      expect(others(b.awareness)).toHaveLength(1);
    });
    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(others(a.awareness)).toEqual([]);

    // Let write, the room is told the caret they hold; made a reader again, it is gone.
    await backend.setMemberRole(boardId, other.userId, 'editor');
    await vi.waitFor(() => {
      expect(others(a.awareness)).toMatchObject([{ user: { id: other.userId } }]);
    });
    await backend.setMemberRole(boardId, other.userId, 'viewer');
    await vi.waitFor(() => {
      expect(others(a.awareness)).toEqual([]);
    });
    mine.close();
    theirs.close();
  });

  it('takes a caret away with its reader, and tells who comes in the carets there are', async () => {
    const { backend, other, boardId, cardId } = world();
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const theirs = other.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([caretsOf(mine), caretsOf(theirs)]);
    show(a);
    show(b);
    await vi.waitFor(() => {
      expect(others(a.awareness)).toHaveLength(1);
    });
    theirs.close();
    await vi.waitFor(() => {
      expect(others(a.awareness)).toEqual([]);
    });

    // Who comes in later is told the caret that is there.
    const late = other.openDoc('w', boardId, cardId, hears());
    const c = await caretsOf(late);
    await vi.waitFor(() => {
      expect(others(c.awareness)).toMatchObject([{ user: { id: backend.userId } }]);
    });
    // Taken off the board, their caret goes with them.
    show(c);
    await vi.waitFor(() => {
      expect(others(a.awareness)).toHaveLength(1);
    });
    await backend.removeMember(boardId, other.userId);
    await vi.waitFor(() => {
      expect(others(a.awareness)).toEqual([]);
    });
    mine.close();
    late.close();
  });

  it("tells the rooms of the browser's other tabs, each caret under a number of its own", async () => {
    const first = world();
    const second = world();
    const channel = `caret-test-${crypto.randomUUID()}`;
    first.world.shareRooms = channel;
    second.world.shareRooms = channel;
    const { boardId, cardId } = first;
    // The same person in two tabs: a caret each.
    const here = first.backend.openDoc('w', boardId, cardId, hears());
    const there = second.backend.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([caretsOf(here), caretsOf(there)]);
    show(a);
    show(b);
    await vi.waitFor(() => {
      expect(others(a.awareness)).toMatchObject([{ user: { id: second.backend.userId } }]);
      expect(others(b.awareness)).toMatchObject([{ user: { id: first.backend.userId } }]);
    });
    // A third reader, in the first tab's world, is told both.
    const third = first.other.openDoc('w', boardId, cardId, hears());
    const c = await caretsOf(third);
    await vi.waitFor(() => {
      expect(others(c.awareness)).toHaveLength(2);
    });
    const numbers = [...c.awareness.getStates().keys()].filter((n) => n !== c.awareness.clientID);
    expect(new Set(numbers).size).toBe(2);
    there.close();
    await vi.waitFor(() => {
      expect(others(a.awareness)).toEqual([]);
      expect(others(c.awareness)).toHaveLength(1);
    });
    here.close();
    third.close();
  });

  it('has the editor read the page before each change of the others is in the document, until the editor takes that back', async () => {
    const { backend, other, boardId, cardId } = world();
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const theirs = other.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([caretsOf(mine), caretsOf(theirs)]);
    const lines = (doc: Y.Doc): number => doc.getXmlFragment(FIELD).length;
    const write = (doc: Y.Doc): void => {
      const paragraph = new Y.XmlElement('paragraph');
      doc.getXmlFragment(FIELD).insert(lines(doc), [paragraph]);
    };
    const before = lines(a.doc);
    /** How many lines the document had each time the reading was asked for. */
    const read: number[] = [];
    const leave = mine.state.readsFirst(() => read.push(lines(a.doc)));

    // What the reader writes themselves asks for no reading.
    write(a.doc);
    expect(read).toEqual([]);
    // The other's line: read before it is in.
    write(b.doc);
    await vi.waitFor(() => {
      expect(lines(a.doc)).toBe(before + 2);
    });
    expect(read).toEqual([before + 1]);

    // Another editor took its place, and the first goes away after: the second's reading stays.
    const second: number[] = [];
    const leaveSecond = mine.state.readsFirst(() => second.push(lines(a.doc)));
    leave();
    write(b.doc);
    await vi.waitFor(() => {
      expect(lines(a.doc)).toBe(before + 3);
    });
    expect(read).toEqual([before + 1]);
    expect(second).toEqual([before + 2]);
    // Taken back, nothing is asked.
    leaveSecond();
    write(b.doc);
    await vi.waitFor(() => {
      expect(lines(a.doc)).toBe(before + 4);
    });
    expect(second).toEqual([before + 2]);
    mine.close();
    theirs.close();
  });

  it("holds back the changes the room sends while the reader composes a syllable, and not the others' carets", async () => {
    const { backend, other, boardId, cardId } = world();
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const theirs = other.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([caretsOf(mine), caretsOf(theirs)]);
    const lines = (doc: Y.Doc): number => doc.getXmlFragment(FIELD).length;
    const write = (doc: Y.Doc, text: string): void => {
      const paragraph = new Y.XmlElement('paragraph');
      paragraph.insert(0, [new Y.XmlText(text)]);
      doc.getXmlFragment(FIELD).insert(lines(doc), [paragraph]);
    };
    const pause = (wait: number): Promise<void> =>
      new Promise((resolve) => setTimeout(resolve, wait));
    const before = lines(a.doc);

    // Composing: the other's line waits, their caret does not; the reader's own writing goes out.
    mine.state.composing(true);
    write(b.doc, 'theirs');
    show(b);
    write(a.doc, 'mine');
    await vi.waitFor(() => {
      expect(lines(b.doc)).toBe(before + 2);
      expect(others(a.awareness)).toHaveLength(1);
    });
    await pause(300);
    expect(lines(a.doc)).toBe(before + 1);
    // The syllable is done: what waited is taken in.
    mine.state.composing(false);
    expect(lines(a.doc)).toBe(before + 2);

    // A composition left open holds the others' writing back while it is being typed, each
    // stroke anew, and no longer than two seconds after its last stroke.
    mine.state.composing(true);
    write(b.doc, 'later');
    await pause(HELD_IDLE - 500);
    mine.state.composing(true);
    await pause(HELD_IDLE - 500);
    expect(lines(a.doc)).toBe(before + 2);
    await vi.waitFor(
      () => {
        expect(lines(a.doc)).toBe(before + 3);
      },
      { timeout: 1500 },
    );
    // Quiet, nothing is held: the next change comes in as it is sent.
    write(b.doc, 'at once');
    await vi.waitFor(() => {
      expect(lines(a.doc)).toBe(before + 4);
    });
    mine.close();
    theirs.close();
  }, 15_000);

  it('passes on from another tab only a caret under a number of the rooms', async () => {
    const { world: stored, backend, boardId, cardId } = world();
    const channel = `caret-test-${crypto.randomUUID()}`;
    stored.shareRooms = channel;
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const a = await caretsOf(mine);
    // What another tab's rooms would say of this room: the same card, made the same way.
    const other = world();
    other.world.shareRooms = channel;
    const theirs = other.backend.openDoc('w', boardId, cardId, hears());
    const b = await caretsOf(theirs);
    show(b);
    await vi.waitFor(() => {
      expect(others(a.awareness)).toHaveLength(1);
    });
    const [number] = [...a.awareness.getStates().keys()].filter(
      (client) => client !== a.awareness.clientID,
    );
    expect(number).toBeGreaterThan(2 ** 32);
    // A page of the same origin saying a caret under this reader's own number: not passed on.
    const said = new BroadcastChannel(channel);
    const own = a.awareness.clientID;
    const before = JSON.stringify(a.awareness.getLocalState());
    said.postMessage({
      kind: 'caret',
      cardId,
      tab: tabOf(stored, cardId),
      caret: { number: own, clock: 99, state: '{"user":{"id":"x"},"cursor":null}' },
    });
    // One under a number of a room is: the word of the channel is heard.
    const state = '{"user":{"id":"x"},"cursor":null}';
    said.postMessage({
      kind: 'caret',
      cardId,
      tab: tabOf(stored, cardId),
      caret: { number: 2 ** 40 + 5, clock: 1, state },
    });
    await vi.waitFor(() => {
      expect(a.awareness.getStates().has(2 ** 40 + 5)).toBe(true);
    });
    expect(JSON.stringify(a.awareness.getLocalState())).toBe(before);
    said.close();
    mine.close();
    theirs.close();
  });

  it('tells a tab that opens the card later the caret of a tab that has it open', async () => {
    const first = world();
    const second = world();
    const channel = `caret-test-${crypto.randomUUID()}`;
    first.world.shareRooms = channel;
    second.world.shareRooms = channel;
    const { boardId, cardId } = first;
    const here = first.backend.openDoc('w', boardId, cardId, hears());
    const a = await caretsOf(here);
    show(a);
    await new Promise((resolve) => setTimeout(resolve, 300));
    // The other tab opens the card only now, and the first reader does not move.
    const there = second.backend.openDoc('w', boardId, cardId, hears());
    const b = await caretsOf(there);
    await vi.waitFor(() => {
      expect(others(b.awareness)).toMatchObject([{ user: { id: first.backend.userId } }]);
    });
    here.close();
    there.close();
  });
});
