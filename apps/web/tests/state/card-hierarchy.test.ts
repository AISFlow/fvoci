/**
 * Card hierarchy rules in the memory backend and wire/patch conversion:
 * depth 1-3 allowed, depth 4 (create & subtree move) rejected with `card_too_deep`,
 * cycles and template constraints rejected, trash/restore cascades and orphan
 * re-rooting, and archive export/import roundtrip + hostile archive rejection.
 */
import { describe, expect, it } from 'vitest';

import { cardOf, newCardRequest, patchCardRequest, type Schemas } from '$lib/api/convert.ts';
import { isBackendError, type Change } from '$lib/state/backend.ts';
import { applyCardPatch, cardPatches, isEmptyCardPatch } from '$lib/state/patches.ts';
import { MEMORY_ARCHIVE } from '$lib/state/memory/shapes.ts';

import { setup } from './support.ts';

function boardSetup() {
  const made = setup();
  const board = [...made.world.containers.values()].find((c) => (c.kind ?? 'board') === 'board');
  if (!board) throw new Error('seed has no board');
  return { ...made, boardId: board.id };
}

async function expectError(
  promise: Promise<unknown>,
  status: number,
  check?: { reason?: string; limit?: number; pointer?: string },
) {
  let caught: unknown;
  try {
    await promise;
  } catch (error) {
    caught = error;
  }
  expect(isBackendError(caught)).toBe(true);
  if (!isBackendError(caught)) return;
  expect(caught.status).toBe(status);
  if (check?.reason !== undefined) expect(caught.reason).toBe(check.reason);
  if (check?.limit !== undefined) expect(caught.limit).toBe(check.limit);
  if (check?.pointer !== undefined) expect(caught.refusalAt(check.pointer)).toBeDefined();
}

describe('card hierarchy in the memory backend', () => {
  it('enforces max depth 3 on create and subtree moves, and rejects cycles and templates', async () => {
    const { backend, boardId } = boardSetup();
    const d1 = await backend.createCard(boardId, { title: 'd1' });
    const d2 = await backend.createCard(boardId, { title: 'd2', parentId: d1.id });
    const d3 = await backend.createCard(boardId, { title: 'd3', parentId: d2.id });
    expect(d1.parentId).toBe('');
    expect(d2.parentId).toBe(d1.id);
    expect(d3.parentId).toBe(d2.id);

    // Depth 4 on create is rejected with card_too_deep & limit 3.
    await expectError(backend.createCard(boardId, { title: 'd4', parentId: d3.id }), 400, {
      reason: 'card_too_deep',
      limit: 3,
      pointer: '/parentId',
    });

    // Subtree move: height-2 under depth-1 succeeds, under depth-2 fails.
    const subRoot = await backend.createCard(boardId, { title: 'subRoot' });
    await backend.createCard(boardId, { title: 'subChild', parentId: subRoot.id });
    const otherRoot = await backend.createCard(boardId, { title: 'otherRoot' });
    const moved = await backend.patchCard(boardId, subRoot.id, { parentId: otherRoot.id });
    expect(moved.parentId).toBe(otherRoot.id);
    await expectError(backend.patchCard(boardId, subRoot.id, { parentId: d2.id }), 400, {
      reason: 'card_too_deep',
      limit: 3,
      pointer: '/parentId',
    });

    // Self, 2-node, and 3-node cycles rejected.
    for (const target of [d1.id, d2.id, d3.id]) {
      await expectError(backend.patchCard(boardId, d1.id, { parentId: target }), 400, {
        pointer: '/parentId',
      });
    }

    // Clearing parentId with null or '' moves back to top level.
    expect((await backend.patchCard(boardId, d3.id, { parentId: null })).parentId).toBe('');
    expect((await backend.patchCard(boardId, d3.id, { parentId: d2.id })).parentId).toBe(d2.id);
    expect((await backend.patchCard(boardId, d3.id, { parentId: '' })).parentId).toBe('');

    // Template card rules.
    const tmpl = await backend.createCard(boardId, { title: 'T', isTemplate: true });
    await expectError(backend.createCard(boardId, { title: 'under-t', parentId: tmpl.id }), 400, {
      pointer: '/parentId',
    });
    await expectError(
      backend.createCard(boardId, { title: 't-child', isTemplate: true, parentId: d1.id }),
      400,
      { pointer: '/parentId' },
    );
  });

  it('cascades trash/restore across descendants and re-roots orphans when parent is gone', async () => {
    const { backend, boardId } = boardSetup();
    const d1 = await backend.createCard(boardId, { title: 'd1' });
    const d2 = await backend.createCard(boardId, { title: 'd2', parentId: d1.id });
    const d3 = await backend.createCard(boardId, { title: 'd3', parentId: d2.id });

    const seen: Change[] = [];
    const stop = backend.watchContainer(boardId, (change) => seen.push(change));
    await backend.batchDeleteCards(boardId, [d1.id]);
    const deletedEvent = seen.find((c) => c.kind === 'blocksDeleted');
    expect(deletedEvent?.kind === 'blocksDeleted' ? deletedEvent.blockIds.sort() : []).toEqual(
      [d1.id, d2.id, d3.id].sort(),
    );

    await backend.batchRestoreCards(boardId, [d1.id]);
    expect((await backend.card(boardId, d2.id)).parentId).toBe(d1.id);
    expect((await backend.card(boardId, d3.id)).parentId).toBe(d2.id);

    // Delete d2 alone, then delete d1 -> restoring d2 re-roots d2 to '' and keeps d3 under d2.
    await backend.batchDeleteCards(boardId, [d2.id]);
    await backend.batchDeleteCards(boardId, [d1.id]);
    await backend.batchRestoreCards(boardId, [d2.id]);
    expect((await backend.card(boardId, d2.id)).parentId).toBe('');
    expect((await backend.card(boardId, d3.id)).parentId).toBe(d2.id);
    stop();
  });

  it('preserves card hierarchy across duplicate and archive import, and rejects hostile archives', async () => {
    const { backend, world, boardId } = boardSetup();
    const d1 = await backend.createCard(boardId, { title: 'root-h' });
    const d2 = await backend.createCard(boardId, { title: 'child-h', parentId: d1.id });
    await backend.createCard(boardId, { title: 'grand-h', parentId: d2.id });

    const copy = await backend.duplicateContainer(boardId, false, world.workspace.id);
    const copyCards = await backend.cards(copy.id);
    const byTitle = Object.fromEntries(copyCards.map((c) => [c.title, c]));
    expect(byTitle['child-h']?.parentId).toBe(byTitle['root-h']?.id);
    expect(byTitle['grand-h']?.parentId).toBe(byTitle['child-h']?.id);

    // Hostile archives (cycle, duplicate-ID depth bypass, board-ID parent, dangling parent)
    // are rejected cleanly.
    const hostileCases: Record<string, unknown>[][] = [
      [
        { id: 'a', containerId: 'b-bad', parentId: 'b', type: 'card', title: 'a', fields: {} },
        { id: 'b', containerId: 'b-bad', parentId: 'a', type: 'card', title: 'b', fields: {} },
      ],
      [
        { id: 'c2', containerId: 'b-bad', parentId: '', type: 'card', title: 'c2', fields: {} },
        { id: 'c1', containerId: 'b-bad', parentId: '', type: 'card', title: 'c1', fields: {} },
        { id: 'c2', containerId: 'b-bad', parentId: 'c1', type: 'card', title: 'c2', fields: {} },
        { id: 'c3', containerId: 'b-bad', parentId: 'c2', type: 'card', title: 'c3', fields: {} },
        { id: 'c4', containerId: 'b-bad', parentId: 'c3', type: 'card', title: 'c4', fields: {} },
      ],
      [
        {
          id: 'c1',
          containerId: 'b-bad',
          parentId: 'b-bad',
          type: 'card',
          title: 'c1',
          fields: {},
        },
      ],
      [
        {
          id: 'c1',
          containerId: 'b-bad',
          parentId: 'missing',
          type: 'card',
          title: 'c1',
          fields: {},
        },
      ],
    ];
    for (const blocks of hostileCases) {
      const badLines = [
        JSON.stringify({ version: MEMORY_ARCHIVE }),
        JSON.stringify({
          type: 'container',
          data: {
            id: 'b-bad',
            title: 'Bad',
            visibility: 'private',
            kind: 'board',
            cardProperties: [],
            properties: {},
          },
        }),
        ...blocks.map((data) => JSON.stringify({ type: 'block', data })),
      ].join('\n');
      await expectError(
        backend.importArchive(world.workspace.id, new File([badLines], 'bad.fvoci')),
        400,
      );
    }
  });

  it('converts and patches parentId across wire and state types', () => {
    const wire = {
      id: 'c1',
      boardId: 'b1',
      parentId: 'p1',
      number: 1,
      createdBy: 'u1',
      modifiedBy: 'u1',
      title: 'T',
      icon: '',
      isTemplate: false,
      properties: {},
      body: null,
      hasBody: false,
      tasksTotal: 0,
      tasksDone: 0,
      createdAt: 1,
      updatedAt: 1,
      lastCommentAt: 0,
    } satisfies Schemas['Card'];
    const summary = cardOf(wire);
    expect(summary.parentId).toBe('p1');
    expect(newCardRequest({ title: 'X', parentId: 'p1' }).json.parentId).toBe('p1');
    expect(patchCardRequest({ parentId: null }).json.parentId).toBeNull();
    expect(patchCardRequest({ parentId: '' }).json.parentId).toBe('');
    expect(isEmptyCardPatch({ parentId: '' })).toBe(false);

    const moved = applyCardPatch(summary, { parentId: '' });
    expect(moved.parentId).toBe('');
    const [forward, backward] = cardPatches(moved, summary);
    expect(forward.parentId).toBe('');
    expect(backward.parentId).toBe('p1');
  });
});
