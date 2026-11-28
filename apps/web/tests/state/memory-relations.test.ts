/**
 * Card relations in the in-memory backend, as the server has them: a write
 * is refused for what the server refuses, by the same reason, and a copy of
 * a board names its own cards.
 */
import { describe, expect, it } from 'vitest';

import { BackendError } from '$lib/state/index.ts';
import {
  RELATION_LIMIT,
  relationLink,
  type PropertyValue,
  type RelationLink,
} from '$lib/logic/wire.ts';

import { setup } from './support.ts';

const RELATION = 'after';

async function relatedBoard() {
  const made = setup();
  const { backend } = made;
  const [workspace] = await backend.workspaces();
  const workspaceId = workspace?.id ?? '';
  const [board] = await backend.boards(workspaceId);
  if (!board) throw new Error('no board');
  await backend.patchContainer(board.id, {
    updatedCardProperties: [{ id: RELATION, name: 'After', type: 'relation', options: [] }],
  });
  const card = async (title: string) => (await backend.createCard(board.id, { title })).id;
  return { ...made, workspaceId, boardId: board.id, card };
}

const reasonOf = async (promise: Promise<unknown>): Promise<string | undefined> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) {
      expect(error.status).toBe(400);
      return error.reason;
    }
    throw error;
  }
  return undefined;
};

describe('a relation value written', () => {
  it('is refused, by the reason the server gives, for what the server refuses', async () => {
    const { backend, boardId, card, workspaceId } = await relatedBoard();
    const [first, second] = [await card('First'), await card('Second')];
    const write = (value: unknown) =>
      backend.patchCard(boardId, first, {
        updatedProperties: { [RELATION]: value as PropertyValue },
      });
    const many = Array.from({ length: RELATION_LIMIT + 1 }, (_, at) =>
      relationLink(`c${String(at)}`),
    );
    // A card of another board of the same world: one of a copy of this board.
    const copy = await backend.duplicateContainer(boardId, false, workspaceId);
    const [elsewhere] = await backend.cards(copy.id);

    expect(await reasonOf(write(second))).toBe('relation_not_a_list');
    // Legacy string arrays fail with relation_not_a_list.
    expect(await reasonOf(write([second]))).toBe('relation_not_a_list');
    // Malformed link objects fail with 400 /problems/bad-request (matching Rust InvalidRelationLink).
    for (const bad of [
      [{ card: second, kind: 'bad' }],
      [{ card: second, lagDays: 3651 }],
      [{ card: second, lagDays: null }],
      [{ card: second, extra: 1 }],
      [{ card: '' }],
    ]) {
      const err = await write(bad).catch((caught: unknown) => caught);
      expect(err).toMatchObject({
        status: 400,
        type: '/problems/bad-request',
        errors: [{ pointer: `/updatedProperties/${RELATION}`, reason: 'invalid' }],
      });
    }
    // Writing a relation link list to a non-relation property fails with 400 /problems/bad-request.
    const wrongProp = await backend
      .patchCard(boardId, first, {
        updatedProperties: { note: [relationLink(second)] },
      })
      .catch((caught: unknown) => caught);
    expect(wrongProp).toMatchObject({
      status: 400,
      type: '/problems/bad-request',
      errors: [{ pointer: '/updatedProperties', reason: 'invalid' }],
    });
    expect(await reasonOf(write([relationLink(first)]))).toBe('relation_to_itself');
    expect(await reasonOf(write([relationLink(second), relationLink(second, 'ss', 2)]))).toBe(
      'relation_repeated',
    );
    expect(await reasonOf(write(many))).toBe('relation_too_many');
    expect(await reasonOf(write([relationLink('no-such-card')]))).toBe('relation_card_not_found');
    expect(await reasonOf(write([relationLink(elsewhere?.id ?? '')]))).toBe(
      'relation_card_not_found',
    );
    const [view] = await backend.views(boardId);
    expect(await reasonOf(write([relationLink(view?.id ?? '')]))).toBe('relation_not_a_card');
    // Omitted kind and lagDays normalize to fs / 0 on storage.
    expect(await reasonOf(write([{ card: second }]))).toBeUndefined();
    const stored = (await backend.card(boardId, first)).fields.properties[RELATION];
    expect(stored).toEqual([relationLink(second, 'fs', 0)]);
  });

  it('may name a card in the trash, and keeps naming it with kind and lagDays', async () => {
    const { backend, boardId, card } = await relatedBoard();
    const [first, deleted, third] = [await card('First'), await card('Gone'), await card('Third')];
    await backend.batchDeleteCards(boardId, [deleted]);
    await backend.patchCard(boardId, first, {
      updatedProperties: { [RELATION]: [relationLink(deleted, 'ss', -2)] },
    });
    await backend.patchCard(boardId, first, {
      updatedProperties: {
        [RELATION]: [relationLink(deleted, 'ss', -2), relationLink(third, 'ff', 5)],
      },
    });
    expect((await backend.card(boardId, first)).fields.properties[RELATION]).toEqual([
      relationLink(deleted, 'ss', -2),
      relationLink(third, 'ff', 5),
    ]);
  });
});

describe('a copy of a board', () => {
  it('names the copies of the cards preserving kind and lagDays, and drops what it cannot name so', async () => {
    const { backend, boardId, card, workspaceId } = await relatedBoard();
    const [first, second, deleted] = [
      await card('First'),
      await card('Second'),
      await card('Gone'),
    ];
    await backend.patchCard(boardId, second, {
      updatedProperties: {
        [RELATION]: [relationLink(first, 'sf', -4), relationLink(deleted, 'ss', 2)],
      },
    });
    await backend.batchDeleteCards(boardId, [deleted]);

    const copy = await backend.duplicateContainer(boardId, false, workspaceId);
    const cards = await backend.cards(copy.id);
    const byTitle = (title: string) => cards.find((each) => each.title === title);
    expect(byTitle('Second')?.fields.properties[RELATION]).toEqual([
      relationLink(byTitle('First')?.id ?? '', 'sf', -4),
    ]);
  });
});

describe('a relation value stored before the rules', () => {
  /** A card whose value names itself, a card twice, and a card that never was, as an older server kept it. */
  async function stale() {
    const made = await relatedBoard();
    const { world, card } = made;
    const [first, second] = [await card('First'), await card('Second')];
    const many = Array.from({ length: RELATION_LIMIT }, (_, at) =>
      relationLink(`old-${String(at)}`),
    );
    const stored = world.blocks.get(first);
    if (!stored) throw new Error('no card');
    stored.fields = {
      ...stored.fields,
      properties: {
        [RELATION]: [
          relationLink(first),
          relationLink('never-was'),
          relationLink('never-was'),
          ...many,
        ],
        note: 'before',
      },
    };
    return { ...made, first, second, many };
  }

  it('does not stop a write that leaves it alone', async () => {
    const { backend, boardId, first } = await stale();
    const patched = await backend.patchCard(boardId, first, {
      title: 'Renamed',
      updatedProperties: { note: 'after' },
    });
    expect(patched.fields.properties.note).toBe('after');
  });

  it('may lose cards over the limit, and not ask again for what it named', async () => {
    const { backend, boardId, first, second, many } = await stale();
    const write = (value: RelationLink[]) =>
      backend.patchCard(boardId, first, { updatedProperties: { [RELATION]: value } });
    const error = await write([relationLink('never-was'), ...many, relationLink(second)]).catch(
      (caught: unknown) => caught,
    );
    expect(error).toBeInstanceOf(BackendError);
    expect(error).toMatchObject({
      reason: 'relation_too_many',
      limit: RELATION_LIMIT,
      errors: [{ pointer: '/updatedProperties', reason: 'relation_too_many' }],
    });
    expect(await reasonOf(write([relationLink('never-was'), ...many]))).toBeUndefined();
    expect(
      await reasonOf(write([relationLink('never-was'), relationLink(second)])),
    ).toBeUndefined();
    expect(
      await reasonOf(
        write([relationLink('never-was'), relationLink(second), relationLink('not-either')]),
      ),
    ).toBe('relation_card_not_found');
  });

  it('is made one a write takes in a copy of the board, and clears stale link lists on non-relation properties', async () => {
    const { backend, boardId, workspaceId, world, first, second } = await stale();
    const block = world.blocks.get(first);
    if (!block) throw new Error('no card');
    (block.fields.properties as Record<string, unknown>).note = [relationLink(second, 'ss', 2)];
    const copy = await backend.duplicateContainer(boardId, false, workspaceId);
    const cards = await backend.cards(copy.id);
    const copied = cards.find((each) => each.title === 'First');
    expect(copied?.fields.properties[RELATION]).toEqual([]);
    expect(copied?.fields.properties.note).toBeNull();
  });
});

describe('a new card', () => {
  it('is refused at the member its values are in', async () => {
    const { backend, boardId } = await relatedBoard();
    const error = await backend
      .createCard(boardId, { title: 'New', properties: { [RELATION]: [relationLink('nope')] } })
      .catch((caught: unknown) => caught);
    expect(error).toMatchObject({
      status: 400,
      reason: 'relation_card_not_found',
      type: '/problems/relation-card-not-found',
      errors: [{ pointer: '/properties', reason: 'relation_card_not_found' }],
    });
  });
});
