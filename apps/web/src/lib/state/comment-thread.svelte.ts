/**
 * Shared comment thread store for document-style blocks (wiki pages, course
 * units, and open cards): reads, merges, and orders comments oldest-first,
 * tracks deleted comment IDs across concurrent reads and out-of-order live
 * echoes, handles optimistic-concurrency 409 conflicts (`editComment`), and
 * wires undo/redo for `deleteComment`.
 *
 * It keeps the reactions to the block and to its comments apart from the
 * comments, by block: a comment changed or told again carries none, and keeps
 * its own. What the reader toggles shows at once and goes back if refused;
 * the reader's toggles of one emoji on one block are sent one after another.
 * A read carries the reactions as they were when it was answered: those of a
 * block told since it was asked (a message, a toggle answered) are newer, and
 * it leaves them as they are (`reactionsMark`).
 */
import { SvelteMap } from 'svelte/reactivity';

import type { DocNode } from '../editor/schema.ts';
import type { MemberRole } from '../logic/wire.ts';

import {
  isBackendError,
  isConflict,
  type Comment,
  type CommentVersion,
  type ContainerPermissions,
} from './backend.ts';
import { canChangeComment } from './board-parts.ts';
import type { CommentEdit } from './card-parts.ts';
import { byCreation } from './order.ts';
import { hasReacted, withMine, type Reaction, type WithReactions } from './reaction-types.ts';
import { authorIds } from './user-names.ts';
import type { LoadStatus, Workspace } from './workspace.svelte.ts';
import { reread, take } from './write.ts';

export interface CommentThreadApi {
  list(): Promise<WithReactions<Comment>[]>;
  create(body: DocNode): Promise<Comment>;
  patch(commentId: string, write: { body: DocNode; readAt: number }): Promise<Comment>;
  remove(commentId: string): Promise<void>;
  restore(commentId: string): Promise<Comment | undefined>;
  versions?: (commentId: string) => Promise<CommentVersion[]>;
}

export interface CommentThreadHost {
  readonly workspace: Workspace;
  readonly parentId: string;
  /** The container of the block, which its reactions are asked under. */
  readonly containerId: string;
  role(): MemberRole;
  permissions?: () => ContainerPermissions;
  joinable?: () => boolean;
  isSpeculative?: () => boolean;
  onCommented?: () => void;
  onDeleting?: (commentId: string) => void;
  onConflict?: () => Promise<void>;
  /** A write as the reader, who may have to join the container first. */
  joined?: <T>(write: () => Promise<T>) => Promise<T>;
  readonly api: CommentThreadApi;
}

/** A toggle of the reader's not yet answered: the emoji there (`present`) or not. */
interface Toggle {
  readonly emoji: string;
  readonly present: boolean;
}

/** What a read of the comments starts from, taken as it is asked (`snapshotRead`). */
export interface ReadSnapshot {
  /** The comments shown. */
  readonly beforeIds: readonly string[];
  /** How often each comment was deleted here. */
  readonly deletedBefore: ReadonlyMap<string, number>;
  /** Where the reactions told stood (`reactionsMark`). */
  readonly mark: number;
}

/** What a screen shows and toggles of the reactions to a block and its comments. */
export type ThreadReactions = Pick<
  CommentThreadStore,
  'workspace' | 'canReact' | 'reactionsOf' | 'toggleReaction'
>;

export class CommentThreadStore {
  readonly #host: CommentThreadHost;
  #comments = $state.raw<Comment[]>([]);
  /** Oldest first. */
  readonly comments = $derived(byCreation(this.#comments));
  status = $state<LoadStatus>('idle');
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown
  readonly #deleted = new Map<string, number>();
  #readings = 0;
  #closed = false;
  /** The reactions as last read or told, by block. */
  readonly #told = new SvelteMap<string, readonly Reaction[]>();
  /** The reader's toggles not yet answered, by block, in the order made. */
  readonly #toggles = new SvelteMap<string, readonly Toggle[]>();
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown
  readonly #turns = new Map<string, Promise<unknown>>();
  /** How many times reactions were told as they are now: by a message, or a toggle answered. */
  #toldCount = 0;
  /** By block, the count when its reactions were last told so. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown
  readonly #toldAt = new Map<string, number>();

  constructor(host: CommentThreadHost) {
    this.#host = host;
  }

  get workspace(): Workspace {
    return this.#host.workspace;
  }

  get canComment(): boolean {
    const perms = this.#host.permissions?.();
    if (perms) return perms.canComment;
    const role = this.#host.role();
    return role === 'admin' || role === 'editor' || role === 'commenter';
  }

  canEditComment(createdBy: string): boolean {
    const perms = this.#host.permissions?.();
    if (perms) {
      return (
        perms.canManageOthersComments ||
        (perms.canComment && createdBy === this.#host.workspace.me?.id)
      );
    }
    return canChangeComment(this.#host.role(), createdBy, this.#host.workspace.me?.id);
  }

  canDeleteComment(createdBy: string): boolean {
    return this.canEditComment(createdBy);
  }

  get canReadCommentHistory(): boolean {
    return this.#host.role() !== '' || Boolean(this.#host.joinable?.());
  }

  /**
   * Replaces the comments directly from a parallel initial read (e.g.
   * `CardStore.load`), asked at `mark` (`reactionsMark`).
   */
  setInitial(comments: WithReactions<Comment>[], mark: number): void {
    if (this.#closed) return;
    this.#comments = this.#reactionsTaken(comments, mark);
    this.status = 'ready';
    this.#forgetStrangers();
  }

  /** Snapshots what an external async re-read starts from, taken before it is asked. */
  snapshotRead(): ReadSnapshot {
    return {
      beforeIds: this.#comments.map((comment) => comment.id),
      // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown
      deletedBefore: new Map(this.#deleted),
      mark: this.#toldCount,
    };
  }

  /** Merges re-read comments into what is shown (e.g. `CardStore.refresh`), reconciling restored IDs. */
  mergeRead(read: readonly WithReactions<Comment>[], before: ReadSnapshot): void {
    if (this.#closed) return;
    this.#applyRead(read, before);
  }

  #applyRead(read: readonly WithReactions<Comment>[], before: ReadSnapshot): void {
    const { beforeIds, deletedBefore, mark } = before;
    for (const comment of read) {
      if (deletedBefore.get(comment.id) === this.#deleted.get(comment.id)) {
        this.#deleted.delete(comment.id);
      }
    }
    const comments = this.#reactionsTaken(read, mark).filter(
      (comment) => !this.#deleted.has(comment.id),
    );
    void this.#host.workspace.learnUsers(authorIds(comments));
    this.#comments = reread(this.#comments, comments, beforeIds);
    this.status = 'ready';
    this.#forgetStrangers();
  }

  currentIds(): string[] {
    return this.#comments.map((comment) => comment.id);
  }

  /** The first reading of the comments: a failure is told. */
  read(): Promise<void> {
    return this.#readComments('first');
  }

  /** The comments read again, merged with what is shown: a failure waits for the next change. */
  reread(): Promise<void> {
    return this.#readComments('again');
  }

  async #readComments(kind: 'first' | 'again'): Promise<void> {
    const first = kind === 'first';
    this.#readings += 1;
    const reading = this.#readings;
    const before = this.snapshotRead();
    if (first || this.status === 'failed') this.status = 'loading';
    try {
      const read = await this.#host.api.list();
      if (this.#closed || reading !== this.#readings) return;
      this.#applyRead(read, before);
    } catch (error) {
      if (this.#closed || reading !== this.#readings) return;
      if (isBackendError(error) && error.status === 401) {
        this.#host.workspace.report('load', error);
        return;
      }
      if (this.status !== 'ready') this.status = 'failed';
      const gone = isBackendError(error) && (error.status === 404 || error.status === 403);
      if (first && !gone && !this.#host.isSpeculative?.()) {
        this.#host.workspace.report('load', error);
      }
    }
  }

  close(): void {
    this.#closed = true;
  }

  receive(comment: Comment): void {
    if (comment.parentId !== this.#host.parentId || this.#closed) return;
    if (this.#deleted.has(comment.id)) {
      void this.reread();
      return;
    }
    this.#take(comment);
  }

  #take(comment: Comment): void {
    void this.#host.workspace.learnUsers(authorIds([comment]));
    this.#comments = take(this.#comments, comment);
  }

  #answered(comment: Comment): void {
    if (
      comment.parentId === this.#host.parentId &&
      !this.#closed &&
      !this.#deleted.has(comment.id)
    ) {
      this.#take(comment);
    }
  }

  forget(blockIds: readonly string[]): void {
    for (const id of blockIds) this.#deleted.set(id, (this.#deleted.get(id) ?? 0) + 1);
    this.#comments = this.#comments.filter((comment) => !blockIds.includes(comment.id));
  }

  comment(commentId: string): Comment | undefined {
    return this.#comments.find((existing) => existing.id === commentId);
  }

  async addComment(body: DocNode): Promise<boolean> {
    try {
      this.#answered(await this.#host.api.create(body));
      this.#host.onCommented?.();
      return true;
    } catch (error) {
      this.#host.workspace.report('save', error);
      return false;
    }
  }

  async editComment(commentId: string, body: DocNode, readAt: number): Promise<CommentEdit> {
    try {
      this.#answered(await this.#host.api.patch(commentId, { body, readAt }));
      return 'saved';
    } catch (error) {
      if (!isConflict(error)) {
        this.#host.workspace.report('save', error);
        return 'failed';
      }
      if (this.#host.onConflict) await this.#host.onConflict();
      else await this.reread();
      return 'conflict';
    }
  }

  async deleteComment(commentId: string): Promise<void> {
    const previous = this.comment(commentId);
    try {
      await this.#host.workspace.undo.perform(
        async () => {
          this.#host.onDeleting?.(commentId);
          await this.#host.api.remove(commentId);
          this.forget([commentId]);
        },
        async () => {
          this.#deleted.delete(commentId);
          const restored = await this.#host.api.restore(commentId);
          if (restored) this.#answered(restored);
          else if (previous) this.#answered(previous);
        },
        'deleteComment',
      );
    } catch (error) {
      this.#host.workspace.report('save', error);
    }
  }

  // ---------------------------------------------------------- reactions

  /** Whether the reader may react: who may comment, where the backend keeps reactions. */
  get canReact(): boolean {
    return this.canComment && this.#host.workspace.backend.reactions !== undefined;
  }

  /** The reactions to the block or one of its comments, with the reader's toggles not yet answered. */
  reactionsOf(blockId: string): readonly Reaction[] {
    const me = this.#host.workspace.me?.id;
    let reactions = this.#told.get(blockId) ?? [];
    if (me === undefined) return reactions;
    for (const toggle of this.#toggles.get(blockId) ?? []) {
      reactions = withMine(reactions, toggle.emoji, me, toggle.present);
    }
    return reactions;
  }

  /**
   * Where the reactions told stand: taken before a read is asked, and given
   * with what it carried, so that it does not put older reactions over newer.
   */
  reactionsMark(): number {
    return this.#toldCount;
  }

  /** The reactions a read of the block itself carried (`getCard`, `getPage`, `getUnit`), asked at `mark`. */
  takeReactions(reactions: readonly Reaction[] | undefined, mark: number): void {
    if (this.#closed) return;
    this.#tellRead(this.#host.parentId, reactions ?? [], mark);
  }

  /**
   * `reactionsChanged`: the block's reactions as they are now, if it is this
   * block or one of its comments. While the comments are read, any block's
   * are kept: a comment not yet read may be one of them. Once they are read,
   * or failed to be, those of other blocks go (a read asked later carries
   * them).
   */
  receiveReactions(blockId: string, reactions: readonly Reaction[]): void {
    if (this.#closed) return;
    const settled = this.status === 'ready' || this.status === 'failed';
    if (settled && !this.#knows(blockId)) return;
    this.#tellNow(blockId, reactions);
  }

  /** Adds the reader's `emoji` to the block, or takes it off if it is there. */
  async toggleReaction(blockId: string, emoji: string): Promise<void> {
    const workspace = this.#host.workspace;
    const [me, workspaceId, reactions] = [
      workspace.me?.id,
      workspace.info?.id,
      workspace.backend.reactions,
    ];
    if (me === undefined || workspaceId === undefined || !reactions || this.#isClosed()) return;
    const toggle: Toggle = { emoji, present: !hasReacted(this.reactionsOf(blockId), emoji, me) };
    this.#setToggles(blockId, [...(this.#toggles.get(blockId) ?? []), toggle]);
    const { containerId } = this.#host;
    const send = (): Promise<void> =>
      toggle.present
        ? reactions.add(workspaceId, containerId, blockId, emoji)
        : reactions.remove(workspaceId, containerId, blockId, emoji);
    const joined = this.#host.joined;
    const key = `${blockId} ${emoji}`;
    const turn = (this.#turns.get(key) ?? Promise.resolve()).then(() =>
      joined ? joined(send) : send(),
    );
    const settled = turn.catch(() => undefined);
    this.#turns.set(key, settled);
    try {
      await turn;
      // Kept until a message or a read asked after it tells the reactions as they are now.
      if (!this.#isClosed()) {
        this.#tellNow(blockId, withMine(this.#told.get(blockId) ?? [], emoji, me, toggle.present));
      }
    } catch (error) {
      if (!this.#isClosed()) workspace.report('save', error);
    } finally {
      this.#setToggles(
        blockId,
        (this.#toggles.get(blockId) ?? []).filter((made) => made !== toggle),
      );
      if (this.#turns.get(key) === settled) this.#turns.delete(key);
    }
  }

  /** The comments read without their reactions, which are kept by block; the read was asked at `mark`. */
  #reactionsTaken(read: readonly WithReactions<Comment>[], mark: number): Comment[] {
    return read.map(({ reactions, ...comment }) => {
      this.#tellRead(comment.id, reactions ?? [], mark);
      return comment;
    });
  }

  /** The block's reactions as they are now: newer than those of any read asked before. */
  #tellNow(blockId: string, reactions: readonly Reaction[]): void {
    this.#toldCount += 1;
    this.#toldAt.set(blockId, this.#toldCount);
    this.#tell(blockId, reactions);
  }

  /** The block's reactions as a read asked at `mark` carried them, unless they were told since. */
  #tellRead(blockId: string, reactions: readonly Reaction[], mark: number): void {
    if ((this.#toldAt.get(blockId) ?? 0) > mark) return;
    this.#tell(blockId, reactions);
  }

  /** Whether `blockId` is this block or one of its comments shown. */
  #knows(blockId: string): boolean {
    return blockId === this.#host.parentId || this.comment(blockId) !== undefined;
  }

  /** Forgets what was told of blocks the comments read are not, kept while they were read. */
  #forgetStrangers(): void {
    for (const blockId of [...this.#told.keys()]) {
      if (!this.#knows(blockId)) this.#told.delete(blockId);
    }
    for (const blockId of [...this.#toldAt.keys()]) {
      if (!this.#knows(blockId)) this.#toldAt.delete(blockId);
    }
  }

  #tell(blockId: string, reactions: readonly Reaction[]): void {
    if (reactions.length === 0) {
      this.#told.delete(blockId);
      return;
    }
    this.#told.set(blockId, reactions);
    void this.#host.workspace.learnUsers(reactions.flatMap((reaction) => reaction.userIds));
  }

  #setToggles(blockId: string, toggles: readonly Toggle[]): void {
    if (toggles.length === 0) this.#toggles.delete(blockId);
    else this.#toggles.set(blockId, toggles);
  }

  /** Whether the thread was closed, asked after a wait. */
  #isClosed(): boolean {
    return this.#closed;
  }

  async commentVersions(commentId: string): Promise<CommentVersion[]> {
    if (!this.#host.api.versions) return [];
    const list = await this.#host.api.versions(commentId);
    await this.#host.workspace.learnUsers(
      list.map((version) => version.writtenBy).filter((id) => id !== ''),
    );
    return list;
  }
}
