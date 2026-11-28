/**
 * A board's due date property and its rule of done, as its manager sets
 * them (`components/board-rules/BoardDueRules.svelte`): each a change of
 * the board, undone as one.
 */
import type { DoneRule } from '../logic/wire.ts';

import type { BoardStore } from './board.svelte.ts';

/** Names the board's due date property; none leaves it to the first date property. */
export function setBoardDueProperty(board: BoardStore, propertyId: string | undefined): void {
  void board.changeBoard((current) => {
    const next = { ...current };
    if (propertyId) next.dueDatePropertyId = propertyId;
    else delete next.dueDatePropertyId;
    return next;
  }, 'changeBoard');
}

/** Sets the board's rule of done; none takes it away. */
export function setBoardDoneRule(board: BoardStore, rule: DoneRule | undefined): void {
  void board.changeBoard((current) => {
    const next = { ...current };
    if (rule) next.doneWhen = rule;
    else delete next.doneWhen;
    return next;
  }, 'changeBoard');
}
