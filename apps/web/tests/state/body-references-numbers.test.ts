/**
 * What `#` offers in the bodies of a place (`bodyReferences`): on a board,
 * its own cards by number or title, each said with its number, and never the
 * card whose body it is; in a wiki, nothing, for there is no board there.
 */
import { describe, expect, it } from 'vitest';

import { bodyReferences } from '$lib/features/card/body-references.ts';

import { openFirstBoard } from './support.ts';

async function opened() {
  const made = await openFirstBoard();
  const [alpha, beta] = ['Alpha', 'Beta'].map((title) =>
    made.board.realCards.find((card) => card.title === title),
  );
  if (alpha?.number === undefined || beta?.number === undefined) throw new Error('no cards');
  return {
    ...made,
    alpha: { ...alpha, number: alpha.number },
    beta: { ...beta, number: beta.number },
  };
}

describe('what `#` offers in a body', () => {
  it('on a board, its cards by number, said with the number, and not the card of the body', async () => {
    const { board, alpha, beta } = await opened();
    const numbered = bodyReferences(board, alpha.id).numbered;
    if (!numbered) throw new Error('a board offers its cards');
    expect(numbered(String(beta.number))[0]).toEqual({
      value: beta.id,
      label: `#${String(beta.number)} ${beta.title}`,
    });
    // The card of the body, by its number and by its title: not offered.
    expect(numbered(String(alpha.number)).map((entry) => entry.value)).not.toContain(alpha.id);
    expect(numbered('alpha').map((entry) => entry.value)).toEqual([]);
    // Another card's body offers it.
    expect(bodyReferences(board, beta.id).numbered?.('alpha')).toEqual([
      { value: alpha.id, label: `#${String(alpha.number)} ${alpha.title}` },
    ]);
  });

  it('in a wiki, nothing: there is no board whose cards it would name', async () => {
    const { workspace } = await opened();
    expect(bodyReferences({ workspace }).numbered).toBeUndefined();
  });
});
