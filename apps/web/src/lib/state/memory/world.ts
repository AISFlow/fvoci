/**
 * The data the in-memory backend works on, and the people listening to it.
 *
 * One `World` stands for one server. Several backends can share it, each
 * acting as one user: that is how a test (or the development page) lets
 * "somebody else" change a card while the reader has it open.
 */
import type { DocNode } from '../../editor/schema.ts';
import type { Block, Container, ContainerMember, BoardView, Card } from '../../logic/wire.ts';
import type {
  AccountSettings,
  Category,
  Change,
  ChangeListener,
  CommentVersion,
  PublicLink,
  UserOrganization,
  WorkspaceInfo,
  User,
} from '../backend.ts';

import type { CardActivityLine } from '../activity-types.ts';

import { reads } from './container-access.ts';
import type { KeptOfCard } from './body-versions.ts';
import type { StoredNotification } from './notifications.ts';
import type { Reaction } from '../reaction-types.ts';
import type { ContainerPlace } from './trash.ts';
import type {
  CourseMeeting,
  CourseSchedule,
  MyExamScore,
  UnitNote,
  UnitProgress,
  UnitReview,
} from '../course-types.ts';
import type { TimeEntry } from '../time-types.ts';

/** Stored exam metadata in `World` (individual scores live in `World.examScores`). */
export interface StoredCourseExam {
  id: string;
  containerId: string;
  title: string;
  examDate: string | null;
  maxScore: number;
  description: string;
  createdAt: number;
  updatedAt: number;
  /** The snapshot of the results a manager published: when, its rounded mean, and how many had scored, counted down to fives. */
  published?: { at: number; average: number; bucket: number };
}

/** A view, card, comment or attachment as the world stores it. */
export interface StoredBlock extends Block {
  /** Cards and comments: their text. */
  body?: DocNode | null;
  /** Diary entries: when the body last changed, so metadata-only edits do not stale body `readAt`. */
  bodyUpdatedAt?: number;
  /** Comments whose text was changed: when that last happened. */
  editedAt?: number;
  /** Attachments. */
  file?: { fileId: string; name: string; size: number; contentType: string };
}

/**
 * The order the server lists the blocks of a board in (`ORDER BY created_at,
 * id`), and so copies and exports them: the one made first first, those made
 * at once by their identifiers. The world's own order is the order it stored
 * them in, which a block back from the trash leaves at the end.
 */
export function oldestFirst(a: StoredBlock, b: StoredBlock): number {
  return a.createdAt - b.createdAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
}

export type StoredView = StoredBlock & BoardView;
export type StoredCard = StoredBlock & Card;

export interface StoredFile {
  fileId: string;
  containerId: string;
  name: string;
  size: number;
  contentType: string;
  /** Absent for the files the seed only names: their bytes are not in this repository. */
  blob?: Blob;
  url?: string;
  /** When it was uploaded and by whom, for the files uploaded here. */
  createdAt?: number;
  createdBy?: string;
  /** Its versions, the oldest first, once it was written anew (file-versions.ts). */
  versions?: KeptFileVersion[];
}

/** A version kept of a file: `name`, `size` and `blob` of the file are its newest's. */
export interface KeptFileVersion {
  version: number;
  createdAt: number;
  createdBy: string;
  size: number;
  restoredFrom: number | null;
  blob?: Blob;
}

interface DeletedBlocks {
  containerId: string;
  /** The block that was deleted and everything that went with it. */
  blocks: StoredBlock[];
  deletedAt: number;
  deletedBy: string;
}

interface DeletedContainer {
  container: Container;
  members: ContainerMember[];
  blocks: StoredBlock[];
  deletedAt: number;
  deletedBy: string;
  /** Where each person had filed it. */
  places: ContainerPlace[];
}

interface Watcher {
  /** Undefined: listens to the reader's own things. */
  containerId: string | undefined;
  /** Empty: the world's own (its rooms), told all of the container as the server's are. */
  userId: string;
  listener: ChangeListener;
  /** Of a wiki: whether the watcher reads it, as of the last change told. */
  reads?: boolean;
}

/** A lease of a file held: one editor of it at a time. */
export interface FileLeaseHeld {
  id: string;
  userId: string;
  /** When it ends unless held on to, in milliseconds. */
  until: number;
}

export class World {
  /** The workspace the world's people share; each also has an own space (`spaces.ts`). */
  workspace: WorkspaceInfo = { id: '', title: '' };
  readonly workspaceMembers = new Set<string>();
  /** Each user's own space, by user, once it was asked for. */
  readonly ownSpaces = new Map<string, WorkspaceInfo>();
  /** Shared organizations each user belongs to, when seeded for tests (`spaces.ts`). */
  readonly organizations = new Map<string, UserOrganization[]>();
  /** The password of each user that has one (`spaces.ts`): the world signs nobody in by it. */
  readonly passwords = new Map<string, string>();
  readonly users = new Map<string, User>();
  readonly containers = new Map<string, Container>();
  readonly members = new Map<string, ContainerMember[]>();
  /** Public links of boards, by board. */
  readonly publicLinks = new Map<string, PublicLink>();
  readonly blocks = new Map<string, StoredBlock>();
  readonly files = new Map<string, StoredFile>();
  readonly categories = new Map<string, Category[]>();
  readonly settings = new Map<string, AccountSettings>();
  readonly screenState = new Map<string, Record<string, string>>();
  /** Deleted blocks, by the identifier of the block the deletion was asked for. */
  readonly deletedBlocks = new Map<string, DeletedBlocks>();
  readonly deletedContainers = new Map<string, DeletedContainer>();
  /** The number the next card of each board takes, by board; 1 where none was made (`card-numbers.ts`). */
  readonly nextCardNumbers = new Map<string, number>();
  /** The texts written down of changed comments, by comment, the one it has now last. */
  readonly commentVersions = new Map<string, CommentVersion[]>();
  /** The versions kept of the bodies of cards, by card (`body-versions.ts`). */
  readonly bodyVersions = new Map<string, KeptOfCard>();
  /** What was done to each card, the oldest first (`card-activity.ts`). */
  readonly cardEvents = new Map<string, CardActivityLine[]>();
  /** Who has a file open in the editor, by board and file (`file-versions.ts`). */
  readonly fileLeases = new Map<string, FileLeaseHeld>();
  /** What each user was told, by user (`notifications.ts`). */
  readonly notifications = new Map<string, StoredNotification[]>();
  /** The stars of each user in each workspace, in their order (`userId/workspaceId`, `favorites.ts`). */
  readonly favorites = new Map<string, string[]>();
  /** The reactions to each block that has any, by block (`reactions.ts`). */
  readonly reactions = new Map<string, Reaction[]>();
  /** Explicit unit watch overrides per user (`userId -> Map<unitId, boolean>`). */
  readonly unitWatchers = new Map<string, Map<string, boolean>>();
  /** Personal unit progress rows per user (`userId -> Map<unitId, UnitProgress>`). */
  readonly unitProgress = new Map<string, Map<string, UnitProgress>>();
  /** Personal spaced-repetition unit reviews per user (`unit-reviews.ts`). */
  readonly unitReviews = new Map<string, UnitReview[]>();
  /** Personal study notes per user (`unit-reviews.ts`). */
  readonly unitNotes = new Map<string, UnitNote[]>();
  /** Saved shared course schedules keyed by `courseId` (survive course soft-delete/restore). */
  readonly courseSchedules = new Map<string, CourseSchedule>();
  /** Course study meetings keyed by `courseId`. */
  readonly courseMeetings = new Map<string, CourseMeeting[]>();
  /** Course exam metadata keyed by `courseId`. */
  readonly courseExams = new Map<string, StoredCourseExam[]>();
  /** Personal exam scores keyed by `userId -> Map<examId, MyExamScore>`. */
  readonly examScores = new Map<string, Map<string, MyExamScore>>();
  /** Personal time entries, by user (`time.ts`). */
  readonly timeEntries = new Map<string, TimeEntry[]>();

  /**
   * The name of the channel the rooms of the bodies share with the worlds of
   * the browser's other tabs (`rooms.ts`); none unless set.
   */
  shareRooms: string | undefined;
  /**
   * How long the changes a room sends wait after the last stroke of a
   * syllable being composed, in milliseconds, when not the room client's own
   * (`HELD_IDLE`). A browser test that writes from another tab while one
   * composes sets it to outlast its own steps, which a busy machine slows
   * past those two seconds.
   */
  heldIdle: number | undefined;
  /**
   * Whether the client configuration says that bodies are searched
   * (`searchEnabled`): every server does; false shows a client that is told
   * otherwise, which has no search box.
   */
  /** Whether cross-workspace copies out of the shared organization are allowed (`true` by default). */
  allowCopyOut = true;
  searchEnabled = true;
  /** Before a card's body is written whole: what its room's editors wrote is written into the card (`rooms.ts`). */
  bodyFlushed: ((cardId: string) => void) | undefined;
  /** Whether a card's room holds what its body does not yet (`rooms.ts`). */
  bodyUnwritten: ((cardId: string) => boolean) | undefined;
  /** A card's body was written whole: its room, if it has one, makes its document anew (`rooms.ts`). */
  bodyWritten: ((cardId: string) => void) | undefined;

  readonly #watchers = new Set<Watcher>();
  #counter = 0;
  #clock: () => number;
  #lastTime = 0;

  constructor(clock: () => number = Date.now) {
    this.#clock = clock;
  }

  /** Replaces the clock: a seeded world is built on a made-up one and then joins real time. */
  useClock(clock: () => number): void {
    this.#clock = clock;
  }

  /**
   * An identifier the way the server makes them: a UUID of version 7 in
   * shape. The counter keeps them in the order they were made and makes a
   * seeded world come out the same every time.
   */
  newId(): string {
    this.#counter += 1;
    return `01930000-0000-7000-8000-${this.#counter.toString(16).padStart(12, '0')}`;
  }

  /** The current time in milliseconds, never the same twice: versions are told apart by it. */
  now(): number {
    const time = Math.max(this.#clock(), this.#lastTime + 1);
    this.#lastTime = time;
    return time;
  }

  blocksOf(containerId: string): StoredBlock[] {
    return [...this.blocks.values()].filter((block) => block.containerId === containerId);
  }

  childrenOf(parentId: string): StoredBlock[] {
    return [...this.blocks.values()].filter((block) => block.parentId === parentId);
  }

  watch(watcher: Watcher): () => void {
    if (watcher.containerId !== undefined)
      watcher.reads = reads(this, watcher.userId, watcher.containerId);
    this.#watchers.add(watcher);
    return () => {
      this.#watchers.delete(watcher);
    };
  }

  /**
   * Tells everybody who watches the board. Of a wiki, as the server tells it:
   * its readers are told the change; who stopped reading it with this change
   * (made private, or taken off it) is told that it is gone for them, once;
   * who does not read it is told nothing.
   */
  tellContainer(containerId: string, change: Change): void {
    const kind = this.containers.get(containerId)?.kind;
    if (kind !== 'wiki' && kind !== 'course') {
      this.#tell((watcher) => watcher.containerId === containerId, change);
      return;
    }
    const watching = [...this.#watchers].filter((watcher) => watcher.containerId === containerId);
    const before = new Map(watching.map((watcher) => [watcher, watcher.reads === true]));
    for (const watcher of watching) {
      watcher.reads = watcher.userId === '' || reads(this, watcher.userId, containerId);
    }
    this.#tell((watcher) => watcher.containerId === containerId && watcher.reads === true, change);
    this.#tell((watcher) => before.get(watcher) === true && watcher.reads !== true, {
      kind: 'containerDeleted',
      containerId,
    });
  }

  /**
   * Tells who reads the container as a user, as `tellContainer` does, and not
   * a reader by its public link: what names users (`reactionsChanged`).
   */
  tellReaders(containerId: string, change: Change): void {
    this.#tell(
      (watcher) =>
        watcher.containerId === containerId &&
        watcher.userId !== '' &&
        reads(this, watcher.userId, containerId),
      change,
    );
  }

  /** Tells the given users, wherever they are. */
  tellUsers(userIds: readonly string[], change: Change): void {
    this.#tell(
      (watcher) => watcher.containerId === undefined && userIds.includes(watcher.userId),
      change,
    );
  }

  /** Tells the user, wherever they are, that a board or wiki is gone for them, unless they still read it (an open wiki). */
  tellGone(userId: string, containerId: string): void {
    if (!reads(this, userId, containerId))
      this.tellUsers([userId], { kind: 'containerDeleted', containerId });
  }

  #tell(wanted: (watcher: Watcher) => boolean, change: Change): void {
    const listeners = [...this.#watchers].filter(wanted).map((watcher) => watcher.listener);
    if (listeners.length === 0) return;
    // After the write has answered, like a message on a socket would be. Each
    // listener gets a copy of its own: nobody holds the world's objects.
    queueMicrotask(() => {
      for (const listener of listeners) listener(structuredClone(change));
    });
  }
}
