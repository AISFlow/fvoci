/**
 * The property definitions of a board are a list: a refused write of some of
 * them corrects the undo of a later change of only one, member by member.
 */
import { expect, it, vi } from 'vitest';

import { BackendError } from '$lib/state/index.ts';

import { openFirstBoard } from './support.ts';

it('a later change of one definition is undone to what was stored, after a refused change of two', async () => {
  const { board, backend, workspace } = await openFirstBoard();
  const [first, second] = board.board?.cardProperties ?? [];
  if (!first || !second) throw new Error('the board has fewer than two properties');
  const renamed = (names: Record<string, string>) =>
    board.changeBoard((current) => ({
      ...current,
      cardProperties: current.cardProperties.map((property) => ({
        ...property,
        name: names[property.id] ?? property.name,
      })),
    }));
  let refuse = (): void => undefined;
  const patch = backend.patchContainer.bind(backend);
  vi.spyOn(backend, 'patchContainer')
    .mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          refuse = () => {
            reject(new BackendError(403));
          };
        }),
    )
    .mockImplementation(patch);
  try {
    const refused = renamed({ [first.id]: 'refused one', [second.id]: 'refused two' });
    // Made from what the refused write shows: its undo takes back to that.
    await renamed({ [first.id]: 'later' });
    refuse();
    await refused;
    const names = async () => {
      const stored = await backend.container(board.boardId);
      return [stored, board.board].map((shown) =>
        shown?.cardProperties.slice(0, 2).map((property) => property.name),
      );
    };
    expect(await names()).toEqual([
      ['later', second.name],
      ['later', second.name],
    ]);
    await workspace.undoLast();
    expect(await names()).toEqual([
      [first.name, second.name],
      [first.name, second.name],
    ]);
  } finally {
    refuse();
    workspace.close();
  }
});
