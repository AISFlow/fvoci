/**
 * Which property a view groups by. A board view that names none is grouped
 * by the first property that can group, and a drop on such a view changes
 * that property.
 */
import { describe, expect, it } from 'vitest';

import { openFirstBoard, setup, SMALL, STATUS } from './support.ts';

const view = SMALL.views[0];
if (!view) throw new Error('the small seed has no view');

/** The small board with its view naming no property to group by. */
const UNNAMED = {
  ...SMALL,
  views: [{ ...view, fields: { ...view.fields, groupBy: undefined } }],
};

/** The same view as a table. */
const TABLE = {
  ...SMALL,
  views: [{ ...view, fields: { ...view.fields, viewType: 'table', groupBy: undefined } }],
};

describe('the property a view groups by', () => {
  it('is the one the view names', async () => {
    const { board } = await openFirstBoard();
    const shown = board.orderedViews[0];
    if (!shown) throw new Error('no view');
    expect(board.groupingOf(shown)?.id).toBe(STATUS);
  });

  it('is the first property that can group, for a board view that names none', async () => {
    const { board } = await openFirstBoard(setup([UNNAMED]));
    const shown = board.orderedViews[0];
    if (!shown) throw new Error('no view');
    expect(shown.fields.groupBy).toBeUndefined();
    expect(board.groupingOf(shown)?.id).toBe(STATUS);
    const groups = board.groupsOf(shown, board.cardsOf(shown), (name) => `No ${name}`);
    expect(groups.visible.map((group) => [group.option.label, group.cards.length])).toEqual([
      ['To do', 2],
      ['Doing', 1],
      ['Done', 0],
      ['No Status', 0],
    ]);
  });

  it('is nothing for a table that names none', async () => {
    const { board } = await openFirstBoard(setup([TABLE]));
    const shown = board.orderedViews[0];
    if (!shown) throw new Error('no view');
    expect(board.groupingOf(shown)).toBeUndefined();
  });

  it('is what a drop changes on a board view that names none, and undo takes it back', async () => {
    const { board, workspace } = await openFirstBoard(setup([UNNAMED]));
    const shown = board.orderedViews[0];
    const gamma = board.realCards.find((card) => card.title === 'Gamma');
    if (!shown || !gamma) throw new Error('unexpected seed');
    await board.dropCards({ viewId: shown.id, cardIds: [gamma.id], optionId: 'done', index: 0 });
    expect(board.card(gamma.id)?.fields.properties[STATUS]).toBe('done');
    await workspace.undoLast();
    expect(board.card(gamma.id)?.fields.properties[STATUS]).toBe('doing');
  });
});
