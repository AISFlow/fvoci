/**
 * The quick filters a reader has turned on, board by board
 * (lib/logic/quick-filter.ts). They are the reader's alone: never written to
 * the view or sent to the server.
 *
 * A signed-in reader's are kept in this browser's storage under their
 * account and the board (`fvoci:quick-filters:<user>:<board>`), as the names
 * of the chips and nothing else; another tab that changes them is followed
 * here, and what it changed while no board was shown here is read again when
 * one is. They are forgotten when the reader signs out and when their account
 * goes (src/app/last-workspace.ts). A reader who is not signed in (a public
 * link, whose reader has an empty identifier) neither reads nor writes
 * storage: their chips last for the visit, as anyone's do in a browser
 * without storage.
 *
 * Besides, a card the reader makes while chips are on stays shown until the
 * chips change again or the board is opened again (`made`), so that it does
 * not vanish as it is made because it is nobody's yet or has no due date. It
 * is not given values to meet them, as a view's filter gives a new card. The
 * cards there before are noted when the chips change (here or in another
 * tab) and when the row of chips is first shown on a visit to the board; the
 * cards a view shows read that note and never write it.
 */
import { on } from 'svelte/events';
import { createSubscriber } from 'svelte/reactivity';

import { isQuickFilterId, type QuickFilterId } from '../logic/quick-filter.ts';
import type { Card } from '../logic/wire.ts';

/** The beginning of the key of a reader's chips of a board in `localStorage`. */
export const QUICK_FILTERS_KEY = 'fvoci:quick-filters:';

/** The key of `userId`'s chips of `boardId` in `localStorage`. */
export function quickFiltersKey(userId: string, boardId: string): string {
  return `${QUICK_FILTERS_KEY}${userId}:${boardId}`;
}

/** Where the chips of a reader who is not signed in are, on this page alone. */
const visitKey = (boardId: string): string => `visit:${boardId}`;

/** What the chips note of a board store: its cards, and which visit of it this is. */
export interface VisitedBoard {
  readonly boardId: string;
  /** Counts the visits to the board; another number is another visit. */
  readonly visit?: number;
  readonly realCards: readonly Card[];
}

/** The cards a board had when the chips last changed, or when the visit started. */
interface Noted {
  readonly visit: number | undefined;
  readonly ids: Readonly<Record<string, true>>;
}

export class QuickFilterChoices {
  /**
   * The chips turned on or off on this page, under the key of their reader
   * and board. Those of a signed-in reader that are not here are read from
   * storage every time they are asked for: reading them is no change a
   * derivation could not make. Another tab's change takes them away from
   * here, so that they are read again.
   */
  #chosen = $state.raw<Readonly<Record<string, readonly QuickFilterId[]>>>({});
  readonly #storage: () => Storage | undefined;
  readonly #watch: () => void;
  readonly #noted = new WeakMap<VisitedBoard, Noted>();
  /** The boards noted last, by board: those another tab's change of their chips notes again. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown
  readonly #boards = new Map<string, WeakRef<VisitedBoard>>();
  /**
   * The keys of chips that storage did not take (none, full or blocked):
   * they are this page's alone, and stay on for the visit though storage is
   * read again.
   */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown
  readonly #unkept = new Set<string>();

  constructor(storage: () => Storage | undefined = () => globalThis.localStorage) {
    this.#storage = storage;
    this.#watch = createSubscriber((update) => {
      // Nothing heard storage while nobody asked: another tab may have changed it since.
      this.#drop(null);
      const off = on(window, 'storage', (event) => {
        if (!this.#follows(event)) return;
        this.#renote(event.key);
        this.#drop(event.key);
        update();
      });
      // Nor will anything hear it now: what is asked meanwhile is read from storage.
      return () => {
        off();
        this.#drop(null);
      };
    });
  }

  /** The chips `readerId` turned on for a board, in the order they were. */
  chosen(boardId: string, readerId: string | undefined): readonly QuickFilterId[] {
    if (!readerId) return this.#chosen[visitKey(boardId)] ?? [];
    this.#watch();
    const key = quickFiltersKey(readerId, boardId);
    return this.#chosen[key] ?? this.#read(key);
  }

  /** Turns a chip on, or off when it is on. */
  toggle(board: VisitedBoard, id: QuickFilterId, readerId: string | undefined): void {
    const chosen = this.chosen(board.boardId, readerId);
    const next = chosen.includes(id) ? chosen.filter((other) => other !== id) : [...chosen, id];
    this.#set(board, next, readerId);
  }

  /** Turns off every chip of a board. */
  clear(board: VisitedBoard, readerId: string | undefined): void {
    this.#set(board, [], readerId);
  }

  /** Notes the cards of a board as its row of chips is shown: once a visit. */
  opened(board: VisitedBoard): void {
    const noted = this.#noted.get(board);
    if (!noted || noted.visit !== board.visit) this.#note(board);
  }

  /**
   * Which cards `readerId` made since the chips of the board last changed, or
   * since the visit started: those that were not there then, and that the
   * reader made. Nobody makes a card on a visit they are not signed in to.
   */
  made(board: VisitedBoard, readerId: string | undefined): (card: Card) => boolean {
    const noted = this.#noted.get(board);
    if (!readerId || !noted) return () => false;
    return (card) => card.createdBy === readerId && !Object.hasOwn(noted.ids, card.id);
  }

  /** Forgets every chip of this page, as the reader leaves it (signs out). */
  forget(): void {
    this.#chosen = {};
    this.#unkept.clear();
  }

  #set(board: VisitedBoard, chosen: readonly QuickFilterId[], readerId: string | undefined): void {
    const key = readerId ? quickFiltersKey(readerId, board.boardId) : visitKey(board.boardId);
    this.#chosen = { ...this.#chosen, [key]: chosen };
    this.#note(board);
    if (readerId) this.#keep(key, chosen);
  }

  #note(board: VisitedBoard): void {
    const ids = Object.fromEntries(board.realCards.map((card) => [card.id, true] as const));
    this.#noted.set(board, { visit: board.visit, ids });
    this.#boards.set(board.boardId, new WeakRef(board));
  }

  /**
   * Another tab changed the chips of `key` (all of storage's, when it was
   * cleared): the boards noted with them on this visit are noted again, as
   * when they change here. A card made before then is one the chips judge;
   * one made after is the reader's still. A board on a visit not yet noted
   * is noted when its row is shown. Storage cleared changes no chips that it
   * did not take (`#unkept`): their board is not noted again.
   */
  #renote(key: string | null): void {
    for (const [boardId, held] of this.#boards) {
      const board = held.deref();
      if (!board) {
        this.#boards.delete(boardId);
        continue;
      }
      const ours = key === null ? !this.#unkeptOn(boardId) : key.endsWith(`:${boardId}`);
      if (ours && this.#noted.get(board)?.visit === board.visit) this.#note(board);
    }
  }

  /** Whether chips of `boardId` are among those storage did not take. */
  #unkeptOn(boardId: string): boolean {
    for (const key of this.#unkept) if (key.endsWith(`:${boardId}`)) return true;
    return false;
  }

  /** Whether a change of storage, made by another tab, is of chips kept here. */
  #follows(event: StorageEvent): boolean {
    try {
      if (event.storageArea !== this.#storage()) return false;
    } catch {
      return false;
    }
    return event.key === null || event.key.startsWith(QUICK_FILTERS_KEY);
  }

  /**
   * Lets go of the chips of `key`, to be read again; of all of storage's when
   * `key` is null, but for those storage did not take. Another tab that
   * changed `key` has what it wrote taken here.
   */
  #drop(key: string | null): void {
    if (key !== null) this.#unkept.delete(key);
    const gone = (kept: string): boolean =>
      key === null ? kept.startsWith(QUICK_FILTERS_KEY) && !this.#unkept.has(kept) : kept === key;
    if (!Object.keys(this.#chosen).some(gone)) return;
    this.#chosen = Object.fromEntries(Object.entries(this.#chosen).filter(([kept]) => !gone(kept)));
  }

  #read(key: string): QuickFilterId[] {
    try {
      const kept: unknown = JSON.parse(this.#storage()?.getItem(key) ?? '[]');
      if (!Array.isArray(kept)) return [];
      const known = kept.filter(isQuickFilterId);
      return known.filter((id, at) => known.indexOf(id) === at);
    } catch {
      return [];
    }
  }

  #keep(key: string, chosen: readonly QuickFilterId[]): void {
    try {
      const storage = this.#storage();
      if (storage) {
        if (chosen.length === 0) storage.removeItem(key);
        else storage.setItem(key, JSON.stringify(chosen));
        this.#unkept.delete(key);
        return;
      }
    } catch {
      // Without storage the chips stay on for the visit.
    }
    this.#unkept.add(key);
  }
}

/** The chips of this page, one for every board it opens. */
export const quickFilters = new QuickFilterChoices();
