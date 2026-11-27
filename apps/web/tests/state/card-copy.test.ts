/**
 * A copy of a card on its own board names what its source named, as the
 * server's copy of a card does: the cards of the board, those in its trash
 * too, and nothing that is gone for good.
 */
import { expect, it } from 'vitest';

import { relationLink } from '$lib/logic/wire.ts';

import { openFirstBoard, settle } from './support.ts';

async function related() {
  const opened = await openFirstBoard();
  const { board } = opened;
  const [source, kept, trashed] = board.cards;
  if (!source || !kept || !trashed) throw new Error('three cards are needed');
  await board.changeBoard((current) => ({
    ...current,
    cardProperties: [
      ...current.cardProperties,
      { id: 'rel', name: 'Related', type: 'relation', options: [] },
    ],
  }));
  return { ...opened, source, kept, trashed };
}

it('keeps the cards in the trash that its source named with kind and lagDays', async () => {
  const { board, backend, workspace, source, kept, trashed } = await related();
  const links = [relationLink(trashed.id, 'ss', -2), relationLink(kept.id, 'ff', 4)];
  await board.setCardProperty(source.id, 'rel', links);
  await board.deleteCards([trashed.id]);
  await settle();
  expect(board.card(trashed.id)).toBeUndefined();
  expect((await backend.card(board.boardId, source.id)).fields.properties.rel).toEqual(links);

  const copied = await board.duplicateCard(source.id);
  expect(workspace.problems).toEqual([]);
  expect(copied?.fields.properties.rel).toEqual(links);
});

it('leaves out a card that is gone for good', async () => {
  const { board, world, workspace, source, kept } = await related();
  // As an older server stored it: a card that never was.
  await board.setCardProperty(source.id, 'rel', [relationLink(kept.id, 'sf', 1)]);
  const stored = world.blocks.get(source.id);
  if (!stored) throw new Error('no card');
  stored.fields = {
    ...stored.fields,
    properties: { rel: [relationLink('never-was'), relationLink(kept.id, 'sf', 1)] },
  };
  const copied = await board.duplicateCard(source.id);
  expect(workspace.problems).toEqual([]);
  expect(copied?.fields.properties.rel).toEqual([relationLink(kept.id, 'sf', 1)]);
});
