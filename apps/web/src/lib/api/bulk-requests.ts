/**
 * The requests of `ServerBackend` (`api/backend.ts`) that change many cards
 * of a board in one call: `batchPatchCards`.
 *
 * A card's writes are sent in turn (`BlockTurns`), so that two quick ones do
 * not pass each other on the way. A batch takes its turn on every card it
 * names: it is sent once the writes of those cards sent before it have
 * ended, and a write of one of them sent after it waits for its answer.
 */
import type { BlockTurns } from '../state/write.ts';
import type { CardPropertiesPatch, CardSummary, CardTelling } from '../state/backend.ts';

import type { InWorkspaceFn } from './block-requests.ts';
import { cardOf, type Schemas } from './convert.ts';

/**
 * Runs `write` once the earlier writes of each of `ids` have ended, however
 * they ended; later writes of any of them wait until it has.
 */
export function inTurns<T>(
  turns: BlockTurns,
  ids: readonly string[],
  write: () => Promise<T>,
): Promise<T> {
  let release = (): void => undefined;
  const over = new Promise<void>((resolve) => {
    release = resolve;
  });
  // Every turn is asked for at once: a batch sent later is behind this one on each card.
  const reached = [...new Set(ids)].map(
    (id) =>
      new Promise<void>((resolve) => {
        void turns.run(id, () => {
          resolve();
          return over;
        });
      }),
  );
  return Promise.all(reached)
    .then(write)
    .finally(() => {
      release();
    });
}

export class ServerBulk {
  readonly #inWorkspace: InWorkspaceFn;
  readonly #turns: BlockTurns;

  constructor(inWorkspace: InWorkspaceFn, turns: BlockTurns) {
    this.#inWorkspace = inWorkspace;
    this.#turns = turns;
  }

  batchPatchCards(
    boardId: string,
    cardIds: readonly string[],
    patch: CardPropertiesPatch,
    telling?: CardTelling,
  ): Promise<CardSummary[]> {
    const request: Schemas['CardBatchPatch'] = { cardIds: [...cardIds] };
    const updated = patch.updatedProperties ?? {};
    if (Object.keys(updated).length > 0) request.updatedProperties = updated;
    const deleted = patch.deletedProperties ?? [];
    if (deleted.length > 0) request.deletedProperties = [...deleted];
    return inTurns(this.#turns, cardIds, async () => {
      const cards = await this.#inWorkspace<Schemas['Card'][]>(
        '/boards/{boardId}/cards/batch-patch',
        { boardId },
        {
          method: 'POST',
          json: request,
          query: { disableNotify: telling?.quiet ? 'true' : undefined },
        },
      );
      return cards.map(cardOf);
    });
  }
}
