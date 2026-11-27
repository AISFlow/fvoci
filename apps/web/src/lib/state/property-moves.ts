/**
 * Moves of a board's card properties, one place up or down at a time, each
 * sent as the whole order the server takes (`reorderCardProperties`).
 *
 * A move is worked out against the order as it is when its turn comes, not
 * when it was asked: the moves of one board go one after the other, so two
 * quick ones do not overtake each other, and an undo (the opposite move)
 * still fits after somebody added or deleted a property meanwhile. A move
 * that is refused reads the board again rather than guess what it was.
 */
import { movedProperty, sameOrder, withPropertyOrder } from '../logic/property-order.ts';
import type { Container } from '../logic/wire.ts';

import type { BoardStore } from './board.svelte.ts';

export interface PropertyMovesHost {
  board(): Container | undefined;
  /** Shows the board with the new order at once. */
  show(board: Container): void;
  send(order: string[]): Promise<unknown>;
  /** Reads the board again and shows it, after a refusal. */
  reread(): Promise<void>;
}

export class PropertyMoves {
  readonly #host: PropertyMovesHost;
  #queue: Promise<unknown> = Promise.resolve();

  constructor(host: PropertyMovesHost) {
    this.#host = host;
  }

  /** Moves the property a place up (`-1`) or down (`1`): whether it moved; rejects when refused. */
  move(propertyId: string, step: -1 | 1, among?: readonly string[]): Promise<boolean> {
    const turn = this.#queue.then(() => this.#move(propertyId, step, among));
    this.#queue = turn.catch(() => undefined);
    return turn;
  }

  async #move(propertyId: string, step: -1 | 1, among?: readonly string[]): Promise<boolean> {
    const board = this.#host.board();
    if (!board) return false;
    const ids = board.cardProperties.map((template) => template.id);
    const order = movedProperty(ids, propertyId, step, among);
    if (sameOrder(ids, order)) return false;
    this.#host.show(withPropertyOrder(board, order));
    try {
      await this.#host.send(order);
    } catch (error) {
      await this.#host.reread().catch(() => undefined);
      throw error;
    }
    return true;
  }
}

/** The moves of a board's properties, shown on the board store and sent by its backend. */
export function propertyMovesOf(board: BoardStore): PropertyMoves {
  // Asked each time, as the store does: the workspace's backend is the one of now.
  const backend = (): BoardStore['workspace']['backend'] => board.workspace.backend;
  return new PropertyMoves({
    board: () => board.board,
    show: (shown) => {
      board.board = shown;
    },
    send: (order) => backend().reorderCardProperties(board.boardId, order),
    reread: async () => {
      board.board = await backend().container(board.boardId);
    },
  });
}
