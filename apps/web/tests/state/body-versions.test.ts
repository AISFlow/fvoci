/**
 * The versions of a card's body: the backend in memory keeps them as the
 * server does (before the room is written in, when its writers leave, as
 * the body is written on, around a body written whole) and restores one as
 * a new write of the whole body; the server backend asks for them; a reader
 * by a public link has none; and the reading the dialog shows them from.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { SharedBackend } from '$lib/api/shared-backend.ts';
import { cardVersions, VersionsReading } from '$lib/features/card/body-versions.svelte.ts';
import type { BodyDoc, BodyDocHears } from '$lib/state/backend.ts';
import { BackendError } from '$lib/state/index.ts';
import { VERSION_EVERY, VERSIONS_KEPT } from '$lib/state/memory/body-versions.ts';
import { bodyOf, FIELD, roomsOf } from '$lib/state/memory/rooms.ts';

import { openFirstBoard, serverBackend, setup, text } from './support.ts';

const hears = (): BodyDocHears => ({ state: () => undefined, discarded: () => undefined });

/** The first card of the small board (it has a body), and two editors of its board. */
function world(): ReturnType<typeof setup> & {
  BOARD: string;
  cardId: string;
  me: string;
  them: string;
} {
  const made = setup();
  const card = [...made.world.blocks.values()].find((block) => block.type === 'card' && block.body);
  if (!card) throw new Error('the seed has no card with a body');
  const [me, them] = [made.backend.userId, made.other.userId];
  return { ...made, BOARD: card.containerId, cardId: card.id, me, them };
}

async function joined(body: BodyDoc): Promise<Y.Doc> {
  await vi.waitFor(() => {
    expect(body.state.status).toBe('joined');
  });
  const { doc } = body.state;
  if (!doc) throw new Error('joined without a document');
  return doc;
}

/** The paragraphs of a body, as texts. */
const lines = (body: unknown): string[] =>
  ((body as { content?: { content?: { text?: string }[] }[] } | null)?.content ?? []).map(
    (paragraph) => (paragraph.content ?? []).map((part) => part.text ?? '').join(''),
  );

/**
 * Writes a paragraph at the end of a body's document, once the document has
 * the paragraphs `after` (what another wrote before, so that the order is
 * one), and waits until the room has taken it.
 */
async function type(body: BodyDoc, value: string, after: string[] = []): Promise<void> {
  const doc = await joined(body);
  await vi.waitFor(() => {
    expect(lines(bodyOf(doc))).toEqual(expect.arrayContaining(after));
  });
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

describe('the versions of a body, in memory', () => {
  it('keeps the body before the room is written in, and what was written when its writers leave', async () => {
    const { world: stored, BOARD, backend, other, cardId, me } = world();
    const versions = backend.bodyVersions;
    expect(await versions.list(BOARD, cardId)).toEqual([]);
    const before = stored.blocks.get(cardId)?.updatedAt;

    const mine = backend.openDoc('w', BOARD, cardId, hears());
    const theirs = other.openDoc('w', BOARD, cardId, hears());
    await Promise.all([joined(mine), joined(theirs)]);
    expect(await versions.list(BOARD, cardId)).toEqual([]);
    await type(mine, 'typed');
    // By nobody, as of the card's last change: nobody is known to have written it. It is the
    // card's body no more, though the card is not written yet: what was typed is. Reading
    // the versions writes nothing.
    expect(await versions.list(BOARD, cardId)).toEqual([
      { version: 1, keptAt: before, editors: [], isCurrent: false, cause: 'auto' },
    ]);
    expect((await versions.read(BOARD, cardId, 1)).isCurrent).toBe(false);
    expect(stored.blocks.get(cardId)?.updatedAt).toBe(before);

    // The one who wrote leaves; another who may write stays, and wrote nothing: kept then.
    mine.close();
    await vi.waitFor(async () => {
      expect(await versions.list(BOARD, cardId)).toHaveLength(2);
    });
    expect(roomsOf(stored).members(cardId)).toBe(1);
    theirs.close();
    await vi.waitFor(() => {
      expect(roomsOf(stored).members(cardId)).toBe(0);
    });
    expect(await versions.list(BOARD, cardId)).toHaveLength(2);
    const [newest, first] = await other.bodyVersions.list(BOARD, cardId);
    expect(newest).toMatchObject({ version: 2, editors: [me], isCurrent: true });
    expect(first).toMatchObject({ version: 1, editors: [], isCurrent: false });
    expect(lines((await versions.read(BOARD, cardId, 2)).body)).toEqual(['first body', 'typed']);
    expect(lines((await versions.read(BOARD, cardId, 1)).body)).toEqual(['first body']);
  });

  it('keeps what is written when only readers stay in the room', async () => {
    const { world: stored, BOARD, backend, other, cardId, me } = world();
    await backend.setMemberRole(BOARD, other.userId, 'viewer');
    const mine = backend.openDoc('w', BOARD, cardId, hears());
    const theirs = other.openDoc('w', BOARD, cardId, hears());
    await joined(theirs);
    await type(mine, 'typed');
    mine.close();
    await vi.waitFor(async () => {
      expect(await other.bodyVersions.list(BOARD, cardId)).toHaveLength(2);
    });
    expect((await other.bodyVersions.list(BOARD, cardId))[0]).toMatchObject({ editors: [me] });
    expect(roomsOf(stored).members(cardId)).toBe(1);
    theirs.close();
  });

  it('keeps what two wrote as the last of them leaves, and not as the first does, with one in the room who wrote nothing', async () => {
    const { world: stored, BOARD, backend, other, cardId, me, them } = world();
    const idleId = [...stored.users.keys()].find((id) => id !== me && id !== them);
    if (!idleId) throw new Error('the seed has two people only');
    await backend.setMemberRole(BOARD, idleId, 'editor');
    const versions = backend.bodyVersions;
    const mine = backend.openDoc('w', BOARD, cardId, hears());
    const theirs = other.openDoc('w', BOARD, cardId, hears());
    const idle = backend.as(idleId).openDoc('w', BOARD, cardId, hears());
    await joined(idle);
    await type(mine, 'one');
    await type(theirs, 'two', ['one']);
    mine.close();
    await vi.waitFor(() => {
      expect(roomsOf(stored).members(cardId)).toBe(2);
    });
    expect(await versions.list(BOARD, cardId)).toHaveLength(1);
    theirs.close();
    await vi.waitFor(async () => {
      expect(await versions.list(BOARD, cardId)).toHaveLength(2);
    });
    // Kept by the leaving, not by the room's closing: one is still in it.
    expect(roomsOf(stored).members(cardId)).toBe(1);
    expect((await versions.list(BOARD, cardId))[0]).toMatchObject({
      version: 2,
      editors: [me, them].sort(),
      isCurrent: true,
    });
    idle.close();
  });

  it('keeps nothing as the other who wrote leaves, once the first who wrote has come back', async () => {
    const { world: stored, BOARD, backend, other, cardId, me, them } = world();
    const versions = backend.bodyVersions;
    const mine = backend.openDoc('w', BOARD, cardId, hears());
    const theirs = other.openDoc('w', BOARD, cardId, hears());
    await type(mine, 'one');
    await type(theirs, 'two', ['one']);
    mine.close();
    await vi.waitFor(() => {
      expect(roomsOf(stored).members(cardId)).toBe(1);
    });
    const back = backend.openDoc('w', BOARD, cardId, hears());
    await joined(back);
    theirs.close();
    await vi.waitFor(() => {
      expect(roomsOf(stored).members(cardId)).toBe(1);
    });
    expect(await versions.list(BOARD, cardId)).toHaveLength(1);
    back.close();
    await vi.waitFor(async () => {
      expect(await versions.list(BOARD, cardId)).toHaveLength(2);
    });
    expect((await versions.list(BOARD, cardId))[0]).toMatchObject({ editors: [me, them].sort() });
  });

  it('keeps the body before the first change after the period, as the server, when the card holds what was written', async () => {
    const { world: stored, BOARD, backend, other, cardId, me, them } = world();
    const versions = backend.bodyVersions;
    const mine = backend.openDoc('w', BOARD, cardId, hears());
    const theirs = other.openDoc('w', BOARD, cardId, hears());
    await type(mine, 'one');
    roomsOf(stored).flush();
    expect(await versions.list(BOARD, cardId)).toHaveLength(1);

    const now = Date.now();
    const clock = vi.spyOn(Date, 'now').mockReturnValue(now + VERSION_EVERY);
    await type(theirs, 'two', ['one']);
    roomsOf(stored).flush();
    const kept = await versions.list(BOARD, cardId);
    // The change that came after the period is not in it, nor is its writer.
    expect(kept[0]).toMatchObject({ version: 2, editors: [me], isCurrent: false });
    expect(lines((await versions.read(BOARD, cardId, 2)).body)).toEqual(['first body', 'one']);
    // The period begins again with that change.
    await type(mine, 'three', ['two']);
    roomsOf(stored).flush();
    expect(await versions.list(BOARD, cardId)).toHaveLength(2);
    // The seats keep their order by the clock: it stays where it is until they have left.
    mine.close();
    theirs.close();
    await vi.waitFor(async () => {
      expect(await versions.list(BOARD, cardId)).toHaveLength(3);
    });
    clock.mockRestore();
    expect((await versions.list(BOARD, cardId))[0]).toMatchObject({
      version: 3,
      editors: [me, them].sort(),
    });
  });

  it('keeps a version as the room writes the card after the period, when the card did not hold what was written', async () => {
    const { world: stored, BOARD, backend, other, cardId, me, them } = world();
    const versions = backend.bodyVersions;
    const mine = backend.openDoc('w', BOARD, cardId, hears());
    const theirs = other.openDoc('w', BOARD, cardId, hears());
    await type(mine, 'one');
    expect(await versions.list(BOARD, cardId)).toHaveLength(1);

    const now = Date.now();
    const clock = vi.spyOn(Date, 'now').mockReturnValue(now + VERSION_EVERY);
    await type(theirs, 'two', ['one']);
    roomsOf(stored).flush();
    const kept = await versions.list(BOARD, cardId);
    expect(kept[0]).toMatchObject({ version: 2, editors: [me, them].sort(), isCurrent: true });
    expect(lines((await versions.read(BOARD, cardId, 2)).body)).toEqual([
      'first body',
      'one',
      'two',
    ]);
    clock.mockRestore();
    mine.close();
    theirs.close();
  });

  it('keeps the body before and after a body written whole, the newest ones only', async () => {
    const { world: stored, BOARD, backend, other, cardId, them } = world();
    const versions = backend.bodyVersions;
    const whole = async (value: string): Promise<void> => {
      const readAt = stored.blocks.get(cardId)?.updatedAt ?? 0;
      await other.writeCardBody(BOARD, cardId, { body: text(value), readAt });
    };
    await whole('second');
    expect(await versions.list(BOARD, cardId)).toMatchObject([
      { version: 2, editors: [them], isCurrent: true },
      { version: 1, editors: [], isCurrent: false },
    ]);
    // The body the card has: nothing is kept.
    await whole('second');
    expect(await versions.list(BOARD, cardId)).toHaveLength(2);
    // No body is a version too, once there is one before it.
    const readAt = stored.blocks.get(cardId)?.updatedAt ?? 0;
    await other.writeCardBody(BOARD, cardId, { body: null, readAt });
    expect((await versions.read(BOARD, cardId, 3)).body).toBeNull();

    for (let count = 0; count < VERSIONS_KEPT; count += 1) await whole(`body ${String(count)}`);
    const kept = await versions.list(BOARD, cardId);
    expect(kept).toHaveLength(VERSIONS_KEPT);
    expect(kept.at(-1)?.version).toBe(4);
    expect((await rejection(versions.read(BOARD, cardId, 3))).status).toBe(404);
  });

  it('restores a version as a new write of the whole body: the room gets it, and what was typed stays a version', async () => {
    const { world: stored, BOARD, backend, other, cardId, me, them } = world();
    const versions = backend.bodyVersions;
    const mine = backend.openDoc('w', BOARD, cardId, hears());
    const theirs = other.openDoc('w', BOARD, cardId, hears());
    await Promise.all([joined(mine), joined(theirs)]);
    await type(mine, 'typed');

    await other.bodyVersions.restore(BOARD, cardId, 1);
    expect(lines(stored.blocks.get(cardId)?.body)).toEqual(['first body']);
    expect(stored.blocks.get(cardId)?.modifiedBy).toBe(them);
    // Everyone in the room is in a new document that is the version.
    await vi.waitFor(() => {
      for (const body of [mine, theirs]) {
        expect(body.state.status).toBe('joined');
        expect(lines(body.state.doc && bodyOf(body.state.doc))).toEqual(['first body']);
      }
    });
    expect(await versions.list(BOARD, cardId)).toMatchObject([
      { version: 3, editors: [them], isCurrent: true, cause: 'restored' },
      { version: 2, editors: [me], isCurrent: false, cause: 'auto' },
      { version: 1, editors: [], isCurrent: false, cause: 'auto' },
    ]);
    expect(lines((await versions.read(BOARD, cardId, 2)).body)).toEqual(['first body', 'typed']);

    // The body the card has, by its newest version and by an older one: nothing is written.
    const at = stored.blocks.get(cardId)?.updatedAt;
    await versions.restore(BOARD, cardId, 3);
    await versions.restore(BOARD, cardId, 1);
    expect(stored.blocks.get(cardId)?.updatedAt).toBe(at);
    expect(await versions.list(BOARD, cardId)).toHaveLength(3);
    mine.close();
    theirs.close();
  });

  it('lets members read the versions, and only who edits restore one', async () => {
    const { world: stored, BOARD, backend, other, cardId } = world();
    const readAt = stored.blocks.get(cardId)?.updatedAt ?? 0;
    await backend.writeCardBody(BOARD, cardId, { body: text('second'), readAt });
    // What somebody typed is in the room, not in the card yet: a refused restore writes neither.
    const mine = backend.openDoc('w', BOARD, cardId, hears());
    await type(mine, 'typed');
    const at = stored.blocks.get(cardId)?.updatedAt;
    for (const role of ['viewer', 'commenter'] as const) {
      await backend.setMemberRole(BOARD, other.userId, role);
      expect(await other.bodyVersions.list(BOARD, cardId)).toHaveLength(2);
      expect((await rejection(other.bodyVersions.restore(BOARD, cardId, 1))).status).toBe(403);
    }
    expect(stored.blocks.get(cardId)?.updatedAt).toBe(at);
    mine.close();
    await vi.waitFor(() => {
      expect(lines(stored.blocks.get(cardId)?.body)).toEqual(['second', 'typed']);
    });
    expect((await rejection(backend.bodyVersions.restore(BOARD, cardId, 9))).status).toBe(404);
    expect((await rejection(backend.bodyVersions.list(BOARD, 'nothing'))).status).toBe(404);
    await backend.removeMember(BOARD, other.userId);
    expect((await rejection(other.bodyVersions.list(BOARD, cardId))).status).toBe(404);
    expect((await rejection(other.bodyVersions.read(BOARD, cardId, 1))).status).toBe(404);
    expect((await rejection(other.bodyVersions.restore(BOARD, cardId, 1))).status).toBe(404);
    expect(lines(stored.blocks.get(cardId)?.body)).toEqual(['second', 'typed']);
  });

  it.each(['image', 'file'])(
    'does not restore a version whose %s shows a file that is gone',
    async (kind) => {
      const { world: stored, BOARD, backend, cardId } = world();
      stored.files.set('f1', {
        fileId: 'f1',
        containerId: BOARD,
        name: 'p.png',
        size: 1,
        contentType: '',
      });
      const pictured = {
        type: 'doc',
        content: [{ type: kind, attrs: { fileId: 'f1' } }],
      };
      let readAt = stored.blocks.get(cardId)?.updatedAt ?? 0;
      await backend.writeCardBody(BOARD, cardId, { body: pictured, readAt });
      readAt = stored.blocks.get(cardId)?.updatedAt ?? 0;
      await backend.writeCardBody(BOARD, cardId, { body: text('third'), readAt });
      stored.files.delete('f1');
      expect((await rejection(backend.bodyVersions.restore(BOARD, cardId, 2))).status).toBe(409);
      expect(lines(stored.blocks.get(cardId)?.body)).toEqual(['third']);
    },
  );
});

describe('the versions of a body, from the server', () => {
  /** Answers every request with `answer`, and keeps what was asked. */
  function serve(answer: unknown): string[] {
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
    return sent;
  }

  it('reads the list and a version, and restores one, under the workspace of the card', async () => {
    const version = { version: 2, keptAt: 5, editors: ['u1'], isCurrent: true, cause: 'manual' };
    const versions = serverBackend('').bodyVersions;
    let sent = serve([version]);
    expect(await versions.list('b', 'c')).toEqual([version]);
    expect(sent).toEqual(['GET /api/v1/w/w1/cards/c/body-versions']);

    sent = serve({ ...version, body: text('kept') });
    expect(await versions.read('b', 'c', 2)).toEqual({ ...version, body: text('kept') });
    expect(sent).toEqual(['GET /api/v1/w/w1/cards/c/body-versions/2']);

    sent = serve({ id: 'c' });
    await expect(versions.restore('b', 'c', 2)).resolves.toMatchObject({ id: 'c', body: null });
    expect(sent).toEqual(['POST /api/v1/w/w1/cards/c/body-versions/2/restore']);
  });

  it('has none for a reader by a public link, and restores nothing', async () => {
    const sent = serve([]);
    const versions = new SharedBackend('', 'b1', 't').bodyVersions;
    expect(await versions.list('b1', 'c1')).toEqual([]);
    expect((await rejection(versions.read('b1', 'c1', 1))).status).toBe(403);
    expect((await rejection(versions.restore('b1', 'c1', 1))).status).toBe(403);
    expect(sent).toEqual([]);
  });
});

describe('the reading of the versions of an open card', () => {
  async function reading() {
    const made = await openFirstBoard();
    const card = made.board.cards.find((one) => one.title === 'Alpha');
    if (!card) throw new Error('no card');
    const readAt = made.world.blocks.get(card.id)?.updatedAt ?? 0;
    await made.other.writeCardBody(card.containerId, card.id, { body: text('second'), readAt });
    const open = await made.board.showCard(card.id);
    const learned = vi.spyOn(made.workspace, 'learnUsers');
    const settled = vi.spyOn(open, 'settleBody');
    const versions = new VersionsReading(cardVersions(made.board, open));
    await vi.waitFor(() => {
      expect(versions.status).toBe('ready');
    });
    return { ...made, cardId: card.id, versions, learned, settled };
  }

  it('reads the list with the names of who wrote, a version chosen, and restores it', async () => {
    const { world: stored, workspace, other, cardId, versions, learned, settled } = await reading();
    expect(versions.versions.map((one) => one.version)).toEqual([2, 1]);
    // The names of who wrote are asked for, and what the reader typed is sent before the list is read.
    expect(learned).toHaveBeenCalledWith([other.userId]);
    expect(settled).toHaveBeenCalledTimes(1);
    const [, first] = versions.versions;
    if (!first) throw new Error('no first version');
    await versions.choose(first);
    expect(versions.contentStatus).toBe('ready');
    expect(lines(versions.content?.body)).toEqual(['first body']);
    expect(await versions.restore()).toBe(true);
    expect(settled).toHaveBeenCalledTimes(2);
    expect(lines(stored.blocks.get(cardId)?.body)).toEqual(['first body']);
    workspace.close();
  });

  it('says why a version is not read or restored', async () => {
    const { backend, workspace, versions } = await reading();
    const [, first] = versions.versions;
    if (!first) throw new Error('no first version');
    await versions.choose({ ...first, version: 9 });
    expect(versions.contentStatus).toBe('gone');
    await versions.choose(first);
    for (const [status, refusal] of [
      [404, 'gone'],
      [409, 'unrestorable'],
      [500, 'failed'],
    ] as const) {
      backend.failNext('bodyVersions', new BackendError(status, { detail: 'refused' }));
      expect(await versions.restore()).toBe(false);
      expect(versions.refusal).toBe(refusal);
    }
    versions.leave();
    expect(versions.chosen).toBeUndefined();
    expect(versions.refusal).toBeUndefined();

    backend.failNext('bodyVersions', new BackendError(500, { detail: 'refused' }));
    await versions.read();
    expect(versions.status).toBe('failed');
    workspace.close();
  });
});
