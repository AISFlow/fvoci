/**
 * The writes of a board, its cards and its views. A refused write restores
 * only its own optimistic values: what was changed over it stays. What is
 * sent is a copy (inverses are corrected while requests wait).
 *
 * A write whose answer was lost (no answer came: status 0) may have been
 * stored all the same. Before it is taken for refused (its step leaves the
 * history, the screen takes it back), the thing is read again: when what is
 * stored already says everything the write said, the write was stored, and
 * that reading is its answer. A refusal the server answered is a refusal.
 * What is stored may say the same because somebody else wrote the same
 * meanwhile, and a value a later change replaced cannot be told apart: the
 * write is then taken for stored, and its undo writes back a value the
 * reader did make.
 */
import type { BlockPatch, Container, ContainerPatch, BoardView } from '../logic/wire.ts';

import { isBackendError, type CardPatch, type CardSummary } from './backend.ts';
import type { BoardStore } from './board.svelte.ts';
import { InverseValues, sameValue } from './inverse-values.ts';
import { Inverses } from './inverses.ts';
import { applyContainerPatch, applyCardPatch, applyViewPatch } from './patches.ts';
import { PendingWrites, put, take } from './write.ts';

/** The values of a board's or a card's patch: properties by key, definitions by identifier. */
const propertyValues = new InverseValues('updatedProperties', 'deletedProperties');
const viewValues = new InverseValues('updatedFields', 'deletedFields');

export class BoardWrites {
  readonly #host: BoardStore;
  readonly #pending = new PendingWrites<Container>();
  readonly #inverses = new Inverses<ContainerPatch>('updatedProperties', 'deletedProperties');

  constructor(host: BoardStore) {
    this.#host = host;
  }

  receive(board: Container): boolean {
    return this.#pending.receive(board);
  }

  /** Keeps the inverse of a change; returns what its undo is to wait for (`Inverses.ready`). */
  keep(reverse: ContainerPatch): () => Promise<void> | undefined {
    this.#inverses.keep(this.#host.boardId, reverse);
    return () => this.#inverses.ready(reverse);
  }

  async write(patch: ContainerPatch, reverse: ContainerPatch): Promise<void> {
    const host = this.#host;
    if (!host.board) return;
    // Inverses can be corrected while requests wait. A request already sent has fixed values.
    const written = structuredClone(patch);
    host.board = applyContainerPatch(host.board, written);
    this.#pending.begin(host.boardId);
    let answer: Container | undefined;
    let restore: ContainerPatch = {};
    try {
      answer = await this.#inverses.write(
        host.boardId,
        written,
        reverse,
        (own) => patchBoardOrStored(host, written, own),
        (refused) => {
          restore = refused;
        },
      );
    } finally {
      const latest = this.#pending.end(host.boardId, answer);
      const shown = this.#shown();
      if (shown) {
        // Restore even if a held server version is older than a board put on screen directly.
        const current = answer ? shown : this.#restore(shown, written, restore);
        host.board = latest && latest.updatedAt >= current.updatedAt ? latest : current;
      }
    }
  }

  #shown(): Container | undefined {
    return this.#host.board;
  }

  #restore(shown: Container, written: ContainerPatch, restore: ContainerPatch): Container {
    // Changes outside this writer may have replaced a field too: keep those values.
    const current = propertyValues.read({
      ...shown,
      updatedProperties: shown.properties,
      updatedCardProperties: shown.cardProperties,
    });
    const failed = propertyValues.read(written as Record<string, unknown>);
    const own: Record<string, unknown> = {};
    for (const [key, value] of propertyValues.read(restore as Record<string, unknown>)) {
      if (sameValue(current.get(key), failed.get(key))) propertyValues.put(own, key, value);
    }
    return applyContainerPatch(shown, own);
  }
}

/** How the store shows and sends the writes of its cards, or of its views. */
interface BlockWays<T, P> {
  /** The block as shown, if it is. */
  shown: (id: string) => T | undefined;
  apply: (block: T, patch: P) => T;
  /** Shows a block as it is to be, at once. */
  show: (block: T) => void;
  /** Shows what the backend says of a block (unless what is shown is newer). */
  answered: (block: T) => void;
  /** Sends a write: a `quiet` one was said before (undo, redo), and tells nobody of it. */
  send: (id: string, patch: P, own: (key: string) => boolean, quiet: boolean) => Promise<T>;
}

/** The writes of a board's cards, or of its views: by identifier, as `BoardWrites` for the board. */
export class BlockWrites<T extends { id: string; updatedAt: number }, P extends object> {
  readonly #pending = new PendingWrites<T>();
  readonly #inverses: Inverses<P>;
  readonly #ways: BlockWays<T, P>;

  /** `updated` and `deleted` name the map of values a patch sets and the list it clears. */
  constructor(updated: string, deleted: string, ways: BlockWays<T, P>) {
    this.#inverses = new Inverses(updated, deleted);
    this.#ways = ways;
  }

  /** A version from the backend: true to show it now, false while writes of it are under way. */
  receive(block: T): boolean {
    return this.#pending.receive(block);
  }

  /** Resolves once no write of `id` is under way. */
  settled(id: string): Promise<void> {
    return this.#pending.settled(id);
  }

  /** Keeps the inverse of a change; returns what its undo is to wait for (`Inverses.ready`). */
  keep(id: string, reverse: P): () => Promise<void> | undefined {
    this.#inverses.keep(id, reverse);
    return () => this.#inverses.ready(reverse);
  }

  /**
   * Shows `patch` on the block and sends it; on refusal takes back what is
   * still its own. A `quiet` write was said before (undo, redo): it tells
   * nobody whom it names.
   */
  async write(id: string, patch: P, reverse: P, quiet = false): Promise<void> {
    const ways = this.#ways;
    const current = ways.shown(id);
    if (!current) return;
    const written = structuredClone(patch);
    ways.show(ways.apply(current, written));
    this.#pending.begin(id);
    let restore: P | undefined;
    try {
      const answer = await this.#inverses.write(
        id,
        written,
        reverse,
        (own) => ways.send(id, written, own, quiet),
        (refused) => {
          restore = refused;
        },
      );
      const latest = this.#pending.end(id, answer);
      if (latest) ways.answered(latest);
    } catch (error) {
      const latest = this.#pending.end(id);
      const shown = latest ?? ways.shown(id);
      if (shown) ways.show(latest ?? ways.apply(shown, restore ?? reverse));
      throw error;
    }
  }
}

/** A thing as the values a patch of it names (`InverseValues`). */
type AsPatch<T> = (thing: T) => Record<string, unknown>;

/**
 * `send`'s answer, or, when it was lost, what `read` finds if it holds the
 * write: every value of `patch` that is still the write's on the screen
 * (`own`: no later change replaced it, whatever value that change shows)
 * is stored. A value a later change replaced is not asked for: that change
 * may have been stored over it before the reading.
 */
async function answeredOrStored<T>(
  patch: object,
  values: InverseValues,
  as: AsPatch<T>,
  ways: {
    send: () => Promise<T>;
    read: () => Promise<T | undefined>;
    own: (key: string) => boolean;
  },
): Promise<T> {
  try {
    return await ways.send();
  } catch (error) {
    if (!isBackendError(error) || error.status !== 0) throw error;
    const stored = await ways.read().catch(() => undefined);
    if (stored === undefined) throw error;
    const storedValues = values.read(as(stored));
    for (const [key, value] of values.read(patch as Record<string, unknown>)) {
      if (ways.own(key) && !sameValue(storedValues.get(key), value)) {
        throw error;
      }
    }
    return stored;
  }
}

export function patchBoardOrStored(
  store: BoardStore,
  patch: ContainerPatch,
  own: (key: string) => boolean,
): Promise<Container> {
  const { backend } = store.workspace;
  return answeredOrStored(
    patch,
    propertyValues,
    (board: Container) => ({
      ...board,
      updatedProperties: board.properties,
      updatedCardProperties: board.cardProperties,
    }),
    {
      send: () => backend.patchContainer(store.boardId, patch),
      read: () => backend.container(store.boardId),
      own,
    },
  );
}

export function patchCardOrStored(
  store: BoardStore,
  cardId: string,
  patch: CardPatch,
  own: (key: string) => boolean,
  quiet = false,
): Promise<CardSummary> {
  const { backend } = store.workspace;
  return answeredOrStored(
    patch,
    propertyValues,
    (card: CardSummary) => ({
      parentId: card.parentId,
      title: card.title,
      icon: card.fields.icon,
      recurrence: card.fields.recurrence ?? null,
      updatedProperties: card.fields.properties,
    }),
    {
      send: () => backend.patchCard(store.boardId, cardId, patch, quiet ? { quiet } : undefined),
      read: async () => {
        // Its reactions are the open card's, read with it there.
        const {
          body: _body,
          reactions: _reactions,
          ...card
        } = await backend.card(store.boardId, cardId);
        return card;
      },
      own,
    },
  );
}

export function patchViewOrStored(
  store: BoardStore,
  viewId: string,
  patch: BlockPatch,
  own: (key: string) => boolean,
): Promise<BoardView> {
  const { backend } = store.workspace;
  return answeredOrStored(
    patch,
    viewValues,
    (view: BoardView) => ({ title: view.title, updatedFields: view.fields }),
    {
      send: () => backend.patchView(store.boardId, viewId, patch),
      read: async () => (await backend.views(store.boardId)).find((view) => view.id === viewId),
      own,
    },
  );
}

/** The writes of the cards of `store`: shown at once, answered into its list and its open card. */
export function cardWritesOf(store: BoardStore): BlockWrites<CardSummary, CardPatch> {
  return new BlockWrites('updatedProperties', 'deletedProperties', {
    shown: (cardId: string) => store.card(cardId),
    apply: applyCardPatch<CardSummary>,
    show: (card: CardSummary) => (store.cards = put(store.cards, card)),
    answered: (card: CardSummary) => {
      store.cards = take(store.cards, card);
      store.openCard?.cardChanged(card);
    },
    send: (cardId: string, patch: CardPatch, own: (key: string) => boolean, quiet: boolean) =>
      patchCardOrStored(store, cardId, patch, own, quiet),
  });
}

/** The writes of the views of `store`: shown at once, answered into its list. */
export function viewWritesOf(store: BoardStore): BlockWrites<BoardView, BlockPatch> {
  return new BlockWrites<BoardView, BlockPatch>('updatedFields', 'deletedFields', {
    shown: (viewId) => store.view(viewId),
    apply: applyViewPatch,
    show: (view) => (store.views = put(store.views, view)),
    answered: (view) => (store.views = take(store.views, view)),
    send: (viewId, patch, own) => patchViewOrStored(store, viewId, patch, own),
  });
}
