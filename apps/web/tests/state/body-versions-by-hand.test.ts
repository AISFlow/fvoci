/**
 * A version of a body kept now, by an editor who asks: the backend in memory
 * keeps it as the server does (what the room's editors typed first, not the
 * newest version again, not within a minute of one kept by hand, by who
 * wrote and not who asked), and says how each version came to be kept; the
 * server backend asks for it; a reader by a public link keeps none; and the
 * reading the dialog shows reads the list again in place.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { SharedBackend } from '$lib/api/shared-backend.ts';
import {
  cardVersions,
  keepVersion,
  pageVersions,
  VersionsReading,
} from '$lib/features/card/body-versions.svelte.ts';
import type { BodyDoc, BodyDocHears } from '$lib/state/backend.ts';
import { BackendError, WikiStore, type BoardStore, type CardStore } from '$lib/state/index.ts';
import { BY_HAND_FLOOR, VERSION_EVERY } from '$lib/state/memory/body-versions.ts';
import { FIELD } from '$lib/state/memory/rooms.ts';

import { openFirstBoard, serverBackend, setup, text } from './support.ts';

const hears = (): BodyDocHears => ({ state: () => undefined, discarded: () => undefined });

/** The first card of the small board (it has a body), and the world's clock to move. */
function world() {
  const made = setup();
  const card = [...made.world.blocks.values()].find((block) => block.type === 'card' && block.body);
  if (!card) throw new Error('the seed has no card with a body');
  let later = 0;
  made.world.useClock(() => Date.now() + later);
  const pass = (ms: number): void => {
    later += ms;
  };
  return { ...made, BOARD: card.containerId, cardId: card.id, pass };
}

/** Writes a paragraph at the end of a body's document once it is joined, and waits until the room has it. */
async function type(body: BodyDoc, value: string): Promise<void> {
  await vi.waitFor(() => {
    expect(body.state.status).toBe('joined');
  });
  const doc = body.state.doc;
  if (!doc) throw new Error('joined without a document');
  const paragraph = new Y.XmlElement('paragraph');
  paragraph.insert(0, [new Y.XmlText(value)]);
  const fragment = doc.getXmlFragment(FIELD);
  fragment.insert(fragment.length, [paragraph]);
  await body.settle();
}

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('a version kept by hand, in memory', () => {
  it('keeps what the room holds, by who wrote it, unless it is kept already or one was by hand within a minute', async () => {
    const { world: stored, BOARD, backend, other, cardId, pass } = world();
    const versions = backend.bodyVersions;
    const theirs = other.openDoc('w', BOARD, cardId, hears());
    await type(theirs, 'typed by them');

    const before = stored.now();
    const first = await versions.keep(BOARD, cardId);
    // The body before they began, on its own, then what they typed: theirs, though asked for by me.
    expect(first).toMatchObject({
      outcome: 'kept',
      version: { version: 2, cause: 'manual', editors: [other.userId], isCurrent: true },
    });
    expect(first.version?.keptAt).toBeGreaterThan(before);
    expect(await versions.keep(BOARD, cardId)).toMatchObject({
      outcome: 'unchanged',
      version: { version: 2 },
    });
    await type(theirs, 'and more');
    expect(await versions.keep(BOARD, cardId)).toMatchObject({
      outcome: 'recent',
      version: { version: 2, cause: 'manual' },
    });
    pass(BY_HAND_FLOOR + 1);
    // What they typed since is theirs; the one who asks, having typed nothing, is not named.
    expect(await versions.keep(BOARD, cardId)).toMatchObject({
      outcome: 'kept',
      version: { version: 3, cause: 'manual', editors: [other.userId] },
    });
    expect((await versions.list(BOARD, cardId)).map((one) => one.cause)).toEqual([
      'manual',
      'manual',
      'auto',
    ]);
    theirs.close();
  });

  it('keeps by hand the version the interval has made due, and leaves the interval be when it keeps none', async () => {
    const { BOARD, backend, other, cardId } = world();
    const versions = backend.bodyVersions;
    vi.useFakeTimers({ toFake: ['Date'], now: Date.now() });
    try {
      const theirs = other.openDoc('w', BOARD, cardId, hears());
      await type(theirs, 'typed by them');
      vi.setSystemTime(Date.now() + VERSION_EVERY + 1);
      await type(theirs, 'and later');
      // Before the room writes on its own: the version due is the one asked for, not one before it.
      expect(await versions.keep(BOARD, cardId)).toMatchObject({
        outcome: 'kept',
        version: { version: 2, cause: 'manual' },
      });
      // Typed again, and asked again within the minute: none is kept, and the interval
      // counts from what was typed, so typing on keeps none on its own.
      await type(theirs, 'then more');
      expect((await versions.keep(BOARD, cardId)).outcome).toBe('recent');
      await type(theirs, 'and more again');
      expect((await versions.list(BOARD, cardId)).map((one) => one.cause)).toEqual([
        'manual',
        'auto',
      ]);
      theirs.close();
    } finally {
      vi.useRealTimers();
    }
  });

  it('leaves nothing to keep as its writers leave, until they write again', async () => {
    const { BOARD, backend, cardId } = world();
    const versions = backend.bodyVersions;
    const mine = backend.openDoc('w', BOARD, cardId, hears());
    await type(mine, 'typed');
    await versions.keep(BOARD, cardId);
    mine.close();
    // Written on again and left: that is kept on its own, and nothing came of the first leaving.
    const again = backend.openDoc('w', BOARD, cardId, hears());
    await type(again, 'again');
    again.close();
    await vi.waitFor(async () => {
      expect(await versions.list(BOARD, cardId)).toHaveLength(3);
    });
    expect((await versions.list(BOARD, cardId)).map((one) => one.cause)).toEqual([
      'auto',
      'manual',
      'auto',
    ]);
  });

  it('keeps the body a version is gone back to as restored, and the body before on its own', async () => {
    const { world: stored, BOARD, backend, cardId } = world();
    const readAt = stored.blocks.get(cardId)?.updatedAt ?? 0;
    await backend.writeCardBody(BOARD, cardId, { body: text('second'), readAt });
    await backend.bodyVersions.restore(BOARD, cardId, 1);
    expect(
      (await backend.bodyVersions.list(BOARD, cardId)).map((one) => [one.version, one.cause]),
    ).toEqual([
      [3, 'restored'],
      [2, 'auto'],
      [1, 'auto'],
    ]);
  });

  it('is for who edits the board: who reads or comments is refused, who is not on it does not find the card', async () => {
    const { world: stored, BOARD, backend, other, cardId } = world();
    const at = stored.blocks.get(cardId)?.updatedAt;
    for (const role of ['viewer', 'commenter'] as const) {
      await backend.setMemberRole(BOARD, other.userId, role);
      expect((await rejection(other.bodyVersions.keep(BOARD, cardId))).status).toBe(403);
    }
    expect((await rejection(backend.bodyVersions.keep(BOARD, 'nothing'))).status).toBe(404);
    await backend.removeMember(BOARD, other.userId);
    expect((await rejection(other.bodyVersions.keep(BOARD, cardId))).status).toBe(404);
    expect(stored.blocks.get(cardId)?.updatedAt).toBe(at);
    expect(await backend.bodyVersions.list(BOARD, cardId)).toEqual([]);
  });
});

describe('a version kept by hand, from the server', () => {
  it('is asked for under the workspace, of a card and of a page', async () => {
    const answer = { outcome: 'kept', version: null };
    const sent: string[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      sent.push(`${init.method ?? 'GET'} ${url}`);
      return Promise.resolve(
        new Response(JSON.stringify(answer), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    const backend = serverBackend('');
    expect(await backend.bodyVersions.keep('b', 'c')).toEqual(answer);
    expect(await backend.wiki.bodyVersions.keep('w9', 'p')).toEqual(answer);
    expect(sent).toEqual([
      'POST /api/v1/w/w1/cards/c/body-versions',
      'POST /api/v1/w/w1/wikis/w9/pages/p/body-versions',
    ]);
  });

  it('is none for a reader by a public link', async () => {
    const fetched = vi.fn();
    vi.stubGlobal('fetch', fetched);
    const versions = new SharedBackend('', 'b1', 't').bodyVersions;
    expect((await rejection(versions.keep('b1', 'c1'))).status).toBe(403);
    expect(fetched).not.toHaveBeenCalled();
  });
});

/** A card of the board shown, with its body's room joined to write in. */
async function joined(board: BoardStore, cardId: string): Promise<CardStore> {
  const open = await board.showCard(cardId);
  open.openBody();
  await vi.waitFor(() => {
    expect(open.room?.writable).toBe(true);
  });
  return open;
}

describe('a version kept by hand, from the dialog', () => {
  it('reads the list again in place, the version being read with it', async () => {
    const made = await openFirstBoard();
    const card = made.board.cards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('no card');
    const readAt = made.world.blocks.get(card.id)?.updatedAt ?? 0;
    await made.other.writeCardBody(card.containerId, card.id, { body: text('second'), readAt });
    const open = await joined(made.board, card.id);
    const source = cardVersions(made.board, open);
    const reading = new VersionsReading(source);
    await vi.waitFor(() => {
      expect(reading.status).toBe('ready');
    });
    const [newest] = reading.versions;
    if (!newest) throw new Error('no version');
    await reading.choose(newest);
    expect(reading.content?.isCurrent).toBe(true);

    const settled = vi.spyOn(open, 'settleBody');
    const statuses: string[] = [];
    const read = vi.spyOn(source.versions, 'list').mockImplementation(async (...args) => {
      statuses.push(reading.status);
      read.mockRestore();
      return source.versions.list(...args);
    });
    // The body is the newest version: nothing is kept, and the list stays as it was.
    await reading.keep();
    expect(reading.kept).toBe('unchanged');
    expect(settled).toHaveBeenCalled();
    expect(statuses).toEqual(['ready']);
    expect(reading.versions.map((one) => one.version)).toEqual([2, 1]);
    expect(reading.chosen?.version).toBe(2);

    made.backend.failNext('bodyVersions', new BackendError(500, { detail: 'refused' }));
    await reading.keep();
    expect(reading.kept).toBe('failed');
    expect(reading.status).toBe('ready');
    made.workspace.close();
  });

  it('says that a version was not kept when the server could not be asked', async () => {
    const made = await openFirstBoard();
    const card = made.board.cards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('no card');
    const open = await joined(made.board, card.id);
    made.backend.failNext('bodyVersions', new BackendError(403, { detail: 'refused' }));
    expect(await keepVersion(cardVersions(made.board, open))).toBe('failed');
    made.workspace.close();
  });

  it('asks for none while what is typed does not go to the room, from the dialog as from the keys', async () => {
    const made = await openFirstBoard();
    const card = made.board.cards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('no card');
    // Not in the room (the card asks for it as it opens: it is left here): what is shown may
    // not be what the card has, and no room is open.
    const open = await made.board.showCard(card.id);
    open.closeBody();
    const source = cardVersions(made.board, open);
    const asked = vi.spyOn(source.versions, 'keep');
    expect(await keepVersion(source)).toBe('notNow');
    const reading = new VersionsReading(source);
    await vi.waitFor(() => {
      expect(reading.status).toBe('ready');
    });
    const read = vi.spyOn(source.versions, 'list');
    // The room being joined: it is being opened.
    open.openBody();
    expect(open.room?.status).toBe('joining');
    await reading.keep();
    expect(reading.kept).toBe('opening');
    // In the room, but no longer to write in it: not for now.
    await vi.waitFor(() => {
      expect(open.room?.writable).toBe(true);
    });
    const { backend, other } = made;
    // Made a commenter by another admin.
    await backend.setMemberRole(made.board.boardId, other.userId, 'admin');
    await other.setMemberRole(made.board.boardId, backend.userId, 'commenter');
    await vi.waitFor(() => {
      expect(open.room).toMatchObject({ status: 'joined', writable: false });
    });
    expect(await keepVersion(source)).toBe('notNow');
    await reading.keep();
    expect(reading.kept).toBe('notNow');
    expect(asked).not.toHaveBeenCalled();
    expect(read).not.toHaveBeenCalled();
    made.workspace.close();
  });

  it('asks for none when the room is lost while what was typed is sent: it may not be in the room', async () => {
    const made = await openFirstBoard();
    const card = made.board.cards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('no card');
    const open = await joined(made.board, card.id);
    let writable = true;
    const source = {
      ...cardVersions(made.board, open),
      writable: () => writable,
      opening: () => false,
      // The connection goes away while what was typed is on its way: the wait gives up.
      settle: () => {
        writable = false;
        return Promise.resolve();
      },
    };
    const asked = vi.spyOn(source.versions, 'keep');
    expect(await keepVersion(source)).toBe('notNow');
    expect(asked).not.toHaveBeenCalled();
    made.workspace.close();
  });

  it('asks for none of a wiki page while it is opened, nor while it cannot be written', async () => {
    const made = setup();
    const wiki = await made.backend.wiki.createWiki(made.world.workspace.id, {
      title: 'Handbook',
      visibility: 'open',
    });
    const created = await made.backend.wiki.createPage(wiki.id, { title: 'Page' });
    await made.workspace.load();
    const store = new WikiStore(made.workspace, wiki.id);
    await store.load();
    const page = store.showPage(created.id);
    if (!page) throw new Error('no page shown');
    const source = pageVersions(page);
    const asked = vi.spyOn(source.versions, 'keep');
    page.openBody();
    expect(await keepVersion(source)).toBe('opening');
    await vi.waitFor(() => {
      expect(page.room?.writable).toBe(true);
    });
    // Asked, now: a page with no body has nothing to keep.
    expect(await keepVersion(source)).toBe('unchanged');
    expect(asked).toHaveBeenCalledTimes(1);
    page.closeBody();
    store.close();
    made.workspace.close();
  });
});
