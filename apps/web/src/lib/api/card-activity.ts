/**
 * The activity of a card on the server (`getCardActivity`), under the
 * workspace of its board.
 */
import type { CardActivity, CardActivityLine } from '../state/activity-types.ts';
import type { Paging } from '../state/trash-reads.ts';

import type { Schemas } from './convert.ts';
import type { RequestOptions } from './http.ts';
import type { WorkspacePath } from './requests.ts';

/** A request under the workspace of the board that `params.boardId` names. */
type Ask = <T>(
  path: WorkspacePath,
  params: Record<string, string>,
  options?: RequestOptions,
) => Promise<T>;

export class ServerCardActivity implements CardActivity {
  readonly #ask: Ask;

  constructor(ask: Ask) {
    this.#ask = ask;
  }

  list(boardId: string, cardId: string, paging?: Paging): Promise<CardActivityLine[]> {
    return this.#ask<Schemas['CardEvent'][]>(
      '/cards/{cardId}/activity',
      { cardId, boardId },
      { query: { page: paging?.page, perPage: paging?.perPage } },
    );
  }
}
