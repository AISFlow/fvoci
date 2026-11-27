/**
 * Keeps each card's `lastCommentAt` current between readings of the card
 * list, as the contract says a client does: a comment that comes is at
 * least as new as the card's last one, and after a comment is deleted the
 * card is read again.
 *
 * Such a reading, and a reading of the whole board again (`begin` and
 * `settle`), may lower a card's time. Any other answer or reading may
 * have been made before a comment that was heard since (a comment does not
 * change a card's `updatedAt`, by which readings are otherwise told apart),
 * so it is taken at the later of the two times. A card in a change message
 * has no time, and keeps the one it had.
 *
 * A deletion message names the block only. The card of a comment is known
 * when the comment came while the board was open, is on the open card, or
 * was deleted by the reader; a block of no card known (a comment that came
 * before the board was read, say) reads the times of all the cards again.
 */
import type { CardSummary, Comment } from './backend.ts';
import type { BoardStore } from './board.svelte.ts';

/** What the board gives to keep the times. */
export interface LastCommentsHost {
  cards(): readonly CardSummary[];
  setCards(cards: CardSummary[]): void;
  /** The card as the server has it now. */
  readCard(cardId: string): Promise<CardSummary>;
  /** The cards as the server has them now. */
  readCards(): Promise<readonly CardSummary[]>;
  /** The blocks the board has now that are not comments: its cards and views, the open card's attachments. */
  blockIds(): string[];
  /** The comments of the open card. */
  openComments(): readonly Comment[];
}

export class LastComments {
  readonly #host: LastCommentsHost;
  /** The card of each comment known, by the comment. */
  readonly #cardOf = new Map<string, string>();
  /** Blocks known not to be comments: a deletion of one leaves the times as they are. */
  readonly #others = new Set<string>();
  /** The newest comment heard of each card that raised its time: when, and at which count. */
  readonly #raised = new Map<string, { at: number; count: number }>();
  /** Comments heard that raised a time, counted; a reading notes the count it began at. */
  #count = 0;
  /** The latest re-reading of each card (`''`: of all): an older one that comes back later is dropped. */
  readonly #readings = new Map<string, number>();

  constructor(host: LastCommentsHost) {
    this.#host = host;
  }

  /** A comment came (posted, changed or brought back): its card's time is at least its creation. */
  heard(comment: Comment): void {
    this.#cardOf.set(comment.id, comment.parentId);
    const cards = this.#host.cards();
    const card = cards.find((each) => each.id === comment.parentId);
    if (!card || (card.lastCommentAt ?? 0) > comment.createdAt) return;
    // Counted also when it is the time the card has (brought back while the card is read again).
    this.#count += 1;
    this.#raised.set(card.id, { at: comment.createdAt, count: this.#count });
    if (card.lastCommentAt === comment.createdAt) return;
    this.#host.setCards(
      cards.map((each) => (each === card ? { ...card, lastCommentAt: comment.createdAt } : each)),
    );
  }

  /** The reader is deleting a block of the board: of a card if it is a comment, `cardId`. */
  deleting(blockId: string, cardId?: string): void {
    if (cardId === undefined) this.#others.add(blockId);
    else this.#cardOf.set(blockId, cardId);
  }

  /** Blocks were deleted: the cards whose last comment may have gone are read again. */
  deleted(blockIds: readonly string[]): void {
    for (const comment of this.#host.openComments()) this.#cardOf.set(comment.id, comment.parentId);
    for (const id of this.#host.blockIds()) this.#others.add(id);
    // What went with a card or a view went with it (the memory backend names them all).
    if (blockIds.some((id) => this.#others.has(id))) return;
    let unknown = false;
    for (const id of blockIds) {
      const cardId = this.#cardOf.get(id);
      if (cardId === undefined) unknown = true;
      else void this.#reread(cardId);
    }
    if (unknown) void this.#reread('');
  }

  /** Cancels any in-flight comment time re-readings (when the board is parked or closed). */
  cancel(): void {
    for (const [key, reading] of this.#readings) this.#readings.set(key, reading + 1);
  }

  /** A mark to tell, when a reading of the board comes back, which comments were heard since it began. */
  begin(): number {
    return this.#count;
  }

  /** A card of a reading of the board begun at `mark`: its time, unless a comment heard since is newer. */
  settle(card: CardSummary, mark: number): CardSummary {
    const raised = this.#raised.get(card.id);
    if (!raised || raised.count <= mark || raised.at <= (card.lastCommentAt ?? 0)) return card;
    return { ...card, lastCommentAt: raised.at };
  }

  /** A card that came another way than a re-reading: never earlier than the time it has. */
  merge(incoming: CardSummary, before: CardSummary | undefined): CardSummary {
    const kept = before?.lastCommentAt;
    if (kept === undefined) return incoming;
    if (incoming.lastCommentAt !== undefined && incoming.lastCommentAt >= kept) return incoming;
    return { ...incoming, lastCommentAt: kept };
  }

  /** Reads the time of a card again, or of every card (`''`), and takes it. */
  async #reread(cardId: string): Promise<void> {
    const reading = (this.#readings.get(cardId) ?? 0) + 1;
    this.#readings.set(cardId, reading);
    const begun = this.#count;
    let read: readonly CardSummary[];
    try {
      read = cardId === '' ? await this.#host.readCards() : [await this.#host.readCard(cardId)];
    } catch {
      // Gone too, or not read: the next reading of the board brings it.
      return;
    }
    if (this.#readings.get(cardId) !== reading) return;
    const times = new Map(read.map((card) => [card.id, card.lastCommentAt ?? 0]));
    const now = this.#host.cards();
    const cards = now.map((card) => {
      const time = times.get(card.id);
      if (time === undefined) return card;
      // A comment heard since the reading began is newer than what it says.
      const raised = this.#raised.get(card.id);
      const at = Math.max(time, raised && raised.count > begun ? raised.at : 0);
      return card.lastCommentAt === at ? card : { ...card, lastCommentAt: at };
    });
    if (cards.some((card, at) => card !== now[at])) this.#host.setCards(cards);
  }
}

/** The times of a board's cards, kept by the board's own lists and its backend. */
export function lastCommentsOf(board: BoardStore): LastComments {
  // Asked each time, as the store does: the workspace's backend is the one of now.
  const backend = (): BoardStore['workspace']['backend'] => board.workspace.backend;
  return new LastComments({
    cards: () => board.cards,
    setCards: (cards) => {
      board.cards = cards;
    },
    readCard: (cardId) => backend().card(board.boardId, cardId),
    readCards: () => backend().cards(board.boardId),
    blockIds: () =>
      [...board.cards, ...board.views, ...(board.openCard?.attachments ?? [])].map(
        (block) => block.id,
      ),
    openComments: () => board.openCard?.comments ?? [],
  });
}
