/**
 * A list of what is in the trash, and bringing one back: the boards of the
 * workspace the reader was an admin of (`containerTrash`), the cards of a board
 * (`cardTrash`, for its editors), the pages of a wiki (`pageTrash`, for its
 * editors), or the entries of a diary (`entryTrash`). Nothing is deleted for good here: the
 * server empties its trash after a while by itself.
 *
 * The list is read in pages, the last deleted first. Bringing one back takes
 * it out of the list, which moves the pages after it; so "more" reads the
 * whole of the list so far again, one page longer, instead of the page after.
 */
import { isBackendError, type DeletedItem, type Paging } from './backend.ts';
import type { BoardStore } from './board.svelte.ts';
import type { CourseStore } from './course.svelte.ts';
import type { DiaryStore } from './diary.svelte.ts';
import { firstOf } from './page-reads.ts';
import type { WikiStore } from './wiki.svelte.ts';
import type { Workspace } from './workspace.svelte.ts';
import { take } from './write.ts';

/** How many a reading adds. */
export const TRASH_PAGE = 50;

export type TrashStatus = 'idle' | 'loading' | 'ready' | 'failed';

export class Trash {
  items = $state.raw<DeletedItem[]>([]);
  status = $state<TrashStatus>('idle');
  /** The last reading was full: there may be more. */
  hasMore = $state(false);
  /** Those on their way back. */
  restoring = $state.raw<string[]>([]);
  /** Those brought back: a reading begun before may still have them. */
  readonly #restored: string[] = [];

  readonly #workspace: Workspace;
  readonly #list: (paging: Paging) => Promise<DeletedItem[]>;
  readonly #restore: (id: string) => Promise<void>;
  /** How many the list asks for. */
  #length = TRASH_PAGE;
  /** Readings begun: the latest is the one that counts. */
  #readings = 0;

  constructor(
    workspace: Workspace,
    list: (paging: Paging) => Promise<DeletedItem[]>,
    restore: (id: string) => Promise<void>,
  ) {
    this.#workspace = workspace;
    this.#list = list;
    this.#restore = restore;
  }

  /** Reads the list from its start. */
  async read(): Promise<void> {
    this.#length = TRASH_PAGE;
    this.items = [];
    await this.#read();
  }

  /** Reads again what was asked for last, after a failure: the pages shown stay asked for. */
  async retry(): Promise<void> {
    if (this.status === 'loading') return;
    await this.#read();
  }

  /** Reads one page more. */
  async more(): Promise<void> {
    if (this.status === 'loading' || !this.hasMore) return;
    this.#length += TRASH_PAGE;
    await this.#read();
  }

  async #read(): Promise<void> {
    this.#readings += 1;
    const reading = this.#readings;
    const length = this.#length;
    this.status = 'loading';
    try {
      const items = await firstOf(this.#list, length);
      if (reading !== this.#readings) return;
      // One on its way back, or back, is not shown again by a reading that began before.
      this.items = items.filter(
        (item) => !this.restoring.includes(item.id) && !this.#restored.includes(item.id),
      );
      this.hasMore = items.length === length;
      this.status = 'ready';
      const deleters = items.map((item) => item.deletedBy).filter((userId) => userId !== '');
      void this.#workspace.learnUsers(deleters);
    } catch (error) {
      if (reading !== this.#readings) return;
      // The list says it failed; a session that is over is the page's to tell.
      this.status = 'failed';
      if (isBackendError(error) && error.status === 401) this.#workspace.report('load', error);
    }
  }

  /**
   * Brings one back. Answers whether it is back; when it is not, the reader
   * is told why, and one the trash no longer has (404) leaves the list.
   */
  async restore(id: string): Promise<boolean> {
    if (this.restoring.includes(id)) return false;
    this.restoring = [...this.restoring, id];
    try {
      await this.#restore(id);
      this.#restored.push(id);
      this.items = this.items.filter((item) => item.id !== id);
      return true;
    } catch (error) {
      if (isBackendError(error) && error.status === 404) {
        this.items = this.items.filter((item) => item.id !== id);
      }
      this.#workspace.report('save', error);
      return false;
    } finally {
      this.restoring = this.restoring.filter((each) => each !== id);
    }
  }
}

/** The boards and wikis of the reader's workspace in the trash that they were an admin of. */
export function containerTrash(workspace: Workspace): Trash {
  return new Trash(
    workspace,
    async (paging) => {
      const info = workspace.info;
      return info ? workspace.backend.deletedContainers(info.id, paging) : [];
    },
    (containerId) => workspace.restoreContainer(containerId),
  );
}

/** The cards of a board in the trash. One that comes back is read again, and shows where it was. */
export function cardTrash(board: BoardStore): Trash {
  return new Trash(
    board.workspace,
    (paging) => board.workspace.backend.deletedCards(board.boardId, paging),
    (cardId) => restoreCard(board, cardId),
  );
}

/** The pages of a wiki in the trash, for its editors. One that comes back shows in its tree. */
export function pageTrash(wiki: WikiStore): Trash {
  return new Trash(
    wiki.workspace,
    (paging) => wiki.deletedPages(paging),
    (pageId) => wiki.restorePage(pageId),
  );
}

/** The entries of a diary in the trash. One that comes back shows on its day. */
export function entryTrash(diary: DiaryStore): Trash {
  return new Trash(
    diary.workspace,
    async (paging) => diary.workspace.backend.diary?.deletedEntries(diary.diaryId, paging) ?? [],
    (entryId) => diary.restoreEntry(entryId),
  );
}

/** The chapters and units of a course in the trash, for its editors. */
export function unitTrash(course: CourseStore): Trash {
  return new Trash(
    course.workspace,
    (paging) => course.deletedUnits(paging),
    (unitId) => course.restoreUnit(unitId),
  );
}

/**
 * Brings a deleted card and any deleted descendants back via `batchRestoreCards`
 * without scanning all cards on the board.
 */
export async function restoreCard(board: BoardStore, cardId: string): Promise<void> {
  const restored = await board.workspace.backend.batchRestoreCards(board.boardId, [cardId]);
  let next = board.cards;
  for (const card of restored) next = take(next, card);
  board.cards = next;
}
