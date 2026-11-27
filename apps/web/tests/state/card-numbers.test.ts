/**
 * The numbers of a board's cards, by each backend. The one in memory numbers
 * as the server does: the cards there are from 1 in the order they were made
 * (as migration 58), each new card the next, none for a template, a copy and
 * an archive's cards new numbers from 1, a card back from the trash its own,
 * and a number never given twice; a card is found by its number on its board
 * only by who may see it. The server backend asks `getCardByNumber` and
 * takes its 404 for none; a reader by a public link asks nothing.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SharedBackend } from '$lib/api/shared-backend.ts';

import { serverBackend, setup } from './support.ts';

afterEach(() => {
  vi.unstubAllGlobals();
});

/** The small world, with the identifiers of its board and of its card Beta. */
function small() {
  const made = setup();
  const cards = [...made.world.blocks.values()].filter((block) => block.type === 'card');
  const beta = cards.find((card) => card.title === 'Beta');
  if (!beta) throw new Error('the seed has no such card');
  return { ...made, boardId: beta.containerId, betaId: beta.id };
}

/** The numbers of the board's cards, by title. */
async function numbersOf(
  backend: ReturnType<typeof setup>['backend'],
  boardId: string,
): Promise<Record<string, number | undefined>> {
  const cards = await backend.cards(boardId);
  return Object.fromEntries(cards.map((card) => [card.title, card.number]));
}

describe('the numbers of cards, in memory', () => {
  it('number the seeded cards in the order they were made, and each new card the next', async () => {
    const { backend, boardId } = small();
    expect(await numbersOf(backend, boardId)).toEqual({ Alpha: 1, Beta: 2, Gamma: 3 });

    const made = await backend.createCard(boardId, { title: 'Delta' });
    const template = await backend.createCard(boardId, { title: 'Plan', isTemplate: true });
    const after = await backend.createCard(boardId, { title: 'Epsilon' });
    expect(made.number).toBe(4);
    // A template has none and takes none: the next card is the one after.
    expect(template.number).toBeUndefined();
    expect(after.number).toBe(5);
    expect((await backend.card(boardId, made.id)).number).toBe(4);
  });

  it('never give a number again, and give a card back from the trash its own', async () => {
    const { backend, boardId } = small();
    const gone = await backend.createCard(boardId, { title: 'Gone' });
    expect(gone.number).toBe(4);
    await backend.deleteBlock(boardId, gone.id);
    expect((await backend.createCard(boardId, { title: 'After' })).number).toBe(5);
    await backend.restoreBlock(boardId, gone.id);
    expect((await numbersOf(backend, boardId)).Gone).toBe(4);
    expect((await backend.createCard(boardId, { title: 'Last' })).number).toBe(6);
  });

  it('give a copy of a board new numbers from 1, and leave the original its own', async () => {
    const { backend, world, boardId } = small();
    await backend.createCard(boardId, { title: 'Plan', isTemplate: true });
    const copy = await backend.duplicateContainer(boardId, false, world.workspace.id);
    const numbers = Object.values(await numbersOf(backend, copy.id));
    expect(numbers.filter((number) => number !== undefined).sort()).toEqual([1, 2, 3]);
    expect(numbers.filter((number) => number === undefined)).toHaveLength(1);
    expect((await backend.createCard(copy.id, { title: 'New' })).number).toBe(4);
    expect(await numbersOf(backend, boardId)).toMatchObject({ Alpha: 1, Beta: 2, Gamma: 3 });
  });

  it('number a copy and an archive in the order the server lists the cards, a card back from the trash in its place', async () => {
    const { backend, world, boardId } = small();
    const alpha = (await backend.cards(boardId)).find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no Alpha');
    // Back from the trash, Alpha is the last the world stored; it was made first.
    await backend.deleteBlock(boardId, alpha.id);
    await backend.restoreBlock(boardId, alpha.id);
    const copy = await backend.duplicateContainer(boardId, false, world.workspace.id);
    expect(await numbersOf(backend, copy.id)).toEqual({ Alpha: 1, Beta: 2, Gamma: 3 });
    const archive = await backend.exportContainer(boardId);
    await backend.importArchive(world.workspace.id, new File([archive.bytes], archive.name));
    const imported = (await backend.boards(world.workspace.id)).find(
      (board) => board.id !== boardId && board.id !== copy.id && board.title === 'Small',
    );
    if (!imported) throw new Error('nothing was imported');
    expect(await numbersOf(backend, imported.id)).toEqual({ Alpha: 1, Beta: 2, Gamma: 3 });
  });

  it('give the cards of an archive new numbers from 1', async () => {
    const { backend, world, boardId } = small();
    await backend.deleteBlock(boardId, (await backend.createCard(boardId, { title: 'Gone' })).id);
    const archive = await backend.exportContainer(boardId);
    const file = new File([archive.bytes], archive.name);
    await backend.importArchive(world.workspace.id, file);
    const imported = (await backend.boards(world.workspace.id)).find(
      (board) => board.id !== boardId && board.title === 'Small',
    );
    if (!imported) throw new Error('nothing was imported');
    expect(await numbersOf(backend, imported.id)).toEqual({ Alpha: 1, Beta: 2, Gamma: 3 });
    expect((await backend.createCard(imported.id, { title: 'Next' })).number).toBe(4);
  });

  it('find a card by its number on its board, and nothing for what is none or not seen', async () => {
    const { backend, other, world, boardId, betaId } = small();
    const workspaceId = world.workspace.id;
    const template = await backend.createCard(boardId, { title: 'Plan', isTemplate: true });
    const gone = await backend.createCard(boardId, { title: 'Gone' });
    await backend.deleteBlock(boardId, gone.id);
    expect(await backend.references.numbered(workspaceId, boardId, 2)).toEqual({
      id: betaId,
      kind: 'card',
      title: 'Beta',
      icon: '',
      containerId: boardId,
      containerKind: 'board',
      number: 2,
    });
    expect(template.number).toBeUndefined();
    expect(await backend.references.numbered(workspaceId, boardId, 4)).toBeUndefined();
    expect(await backend.references.numbered(workspaceId, boardId, 5)).toBeUndefined();
    expect(await backend.references.numbered(workspaceId, 'nowhere', 2)).toBeUndefined();

    // Off the board, a card that is there is told as one that is not.
    expect((await other.references.numbered(workspaceId, boardId, 2))?.id).toBe(betaId);
    await backend.removeMember(boardId, other.userId);
    expect(await other.references.numbered(workspaceId, boardId, 2)).toBeUndefined();
  });

  it('hides a preserved card number while a card is a template across reads, lookup and export, and restores it when ordinary again', async () => {
    const { backend, world, boardId, betaId } = small();
    const workspaceId = world.workspace.id;
    const setTemplate = (id: string, isTemplate: boolean): void => {
      const before = world.blocks.get(id);
      if (!before) throw new Error(`missing block ${id}`);
      world.blocks.set(id, { ...before, fields: { ...before.fields, isTemplate } });
    };

    // Stored block keeps preserved #2 while marked as template; every read/lookup/export hides it.
    setTemplate(betaId, true);
    expect(world.blocks.get(betaId)?.number).toBe(2);
    expect((await backend.card(boardId, betaId)).fields.isTemplate).toBe(true);
    expect((await backend.card(boardId, betaId)).number).toBeUndefined();
    expect((await numbersOf(backend, boardId)).Beta).toBeUndefined();
    expect(await backend.references.numbered(workspaceId, boardId, 2)).toBeUndefined();

    // Archive export omits `number` for the converted template.
    const exported = await (await backend.exportContainer(boardId)).bytes.text();
    const betaLine = exported
      .split('\n')
      .filter((line) => line.trim() !== '')
      .map((line) => JSON.parse(line) as { type?: string; data?: { id?: string; number?: number } })
      .find((entry) => entry.type === 'block' && entry.data?.id === betaId);
    expect(betaLine).toBeDefined();
    expect(betaLine?.data?.number).toBeUndefined();

    // New card takes #4 (never reusing #2).
    const delta = await backend.createCard(boardId, { title: 'Delta' });
    expect(delta.number).toBe(4);

    // Trash and restore while template still keeps the preserved number hidden.
    await backend.deleteBlock(boardId, betaId);
    await backend.restoreBlock(boardId, betaId);
    expect((await backend.card(boardId, betaId)).number).toBeUndefined();

    // Turning it back into an ordinary card shows #2 again without consuming #5.
    setTemplate(betaId, false);
    expect((await backend.card(boardId, betaId)).fields.isTemplate).toBe(false);
    expect((await backend.card(boardId, betaId)).number).toBe(2);
    expect((await backend.references.numbered(workspaceId, boardId, 2))?.id).toBe(betaId);
    expect((await backend.createCard(boardId, { title: 'Epsilon' })).number).toBe(5);
  });
});

describe('the numbers of cards, from the server', () => {
  /** Answers every request with `status` and `answer`, and keeps what was asked. */
  function serve(status: number, answer: unknown): string[] {
    const sent: string[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit = {}) => {
      sent.push(`${init.method ?? 'GET'} ${url}`);
      return Promise.resolve(
        new Response(JSON.stringify(answer), {
          status,
          headers: {
            'Content-Type': status < 400 ? 'application/json' : 'application/problem+json',
          },
        }),
      );
    });
    return sent;
  }

  const card = {
    id: 'c12',
    boardId: 'b',
    title: 'Plan',
    icon: '🚀',
    number: 12,
    properties: {},
    isTemplate: false,
    createdBy: 'u',
    modifiedBy: 'u',
    createdAt: 1,
    updatedAt: 2,
    hasBody: false,
    tasksTotal: 0,
    tasksDone: 0,
  };

  it('asks for a card by its number under the workspace, and takes 404 for none', async () => {
    const { references } = serverBackend('/base');
    const sent = serve(200, card);
    expect(await references.numbered('w1', 'b', 12)).toEqual({
      id: 'c12',
      kind: 'card',
      title: 'Plan',
      icon: '🚀',
      containerId: 'b',
      containerKind: 'board',
      number: 12,
    });
    expect(sent).toEqual(['GET /base/api/v1/w/w1/boards/b/cards/by-number/12']);

    serve(404, { type: '/problems/card', title: 'No card', status: 404 });
    expect(await references.numbered('w1', 'b', 13)).toBeUndefined();
    serve(500, { type: 'about:blank', title: 'Broken', status: 500 });
    await expect(references.numbered('w1', 'b', 12)).rejects.toMatchObject({ status: 500 });
  });

  it('reads a card that is told with an empty number as one with none', async () => {
    const { references } = serverBackend('');
    const linked = { id: 'p', kind: 'page', title: 'Road', icon: '', containerId: 'w' };
    serve(200, [{ ...linked, containerKind: 'wiki', number: null }]);
    expect(await references.search('w1', 'road')).toEqual([{ ...linked, containerKind: 'wiki' }]);
  });

  it('keeps the number of a card of a list', async () => {
    const backend = serverBackend('');
    serve(200, [card, { ...card, id: 'c0', number: null, isTemplate: true }]);
    const cards = await backend.cards('b');
    expect(cards.map((one) => [one.id, one.number])).toEqual([
      ['c12', 12],
      ['c0', undefined],
    ]);
  });
});

describe('the numbers of cards, for a reader by a public link', () => {
  it('find nothing by a number, and ask nothing', async () => {
    const fetched = vi.fn();
    vi.stubGlobal('fetch', fetched);
    const { references } = new SharedBackend('', 'b1', 't');
    expect(await references.numbered('', 'b1', 1)).toBeUndefined();
    expect(fetched).not.toHaveBeenCalled();
  });
});
