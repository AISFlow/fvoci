/**
 * Shared comment thread store for document-style blocks (wiki pages, course
 * units, and open cards): reads, merges, and orders comments oldest-first,
 * tracks deleted comment IDs across concurrent reads and out-of-order live
 * echoes, handles optimistic-concurrency 409 conflicts (`editComment`), and
 * wires undo/redo for `deleteComment`.
 */
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
import { authorIds } from './user-names.ts';
import type { LoadStatus, Workspace } from './workspace.svelte.ts';
import { reread, take } from './write.ts';

export interface CommentThreadApi {
  list(): Promise<Comment[]>;
  create(body: DocNode): Promise<Comment>;
  patch(commentId: string, write: { body: DocNode; readAt: number }): Promise<Comment>;
  remove(commentId: string): Promise<void>;
  restore(commentId: string): Promise<Comment | undefined>;
  versions?: (commentId: string) => Promise<CommentVersion[]>;
}

export interface CommentThreadHost {
  readonly workspace: Workspace;
  readonly parentId: string;
  role(): MemberRole;
  permissions?: () => ContainerPermissions;
  joinable?: () => boolean;
  isSpeculative?: () => boolean;
  onCommented?: () => void;
  onDeleting?: (commentId: string) => void;
  onConflict?: () => Promise<void>;
  readonly api: CommentThreadApi;
}

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

  /** Replaces the comments directly from a parallel initial read (e.g. `CardStore.load`). */
  setInitial(comments: Comment[]): void {
    if (this.#closed) return;
    this.#comments = comments;
    this.status = 'ready';
  }

  /** Snapshots current comment IDs and deletion counters before an external async re-read. */
  snapshotRead(): { beforeIds: string[]; deletedBefore: ReadonlyMap<string, number> } {
    return {
      beforeIds: this.#comments.map((comment) => comment.id),
      // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown
      deletedBefore: new Map(this.#deleted),
    };
  }

  /** Merges re-read comments into what is shown (e.g. `CardStore.refresh`), reconciling restored IDs. */
  mergeRead(
    read: readonly Comment[],
    beforeIds: readonly string[],
    deletedBefore: ReadonlyMap<string, number> = this.#deleted,
  ): void {
    if (this.#closed) return;
    this.#applyRead(read, beforeIds, deletedBefore);
  }

  #applyRead(
    read: readonly Comment[],
    beforeIds: readonly string[],
    deletedBefore: ReadonlyMap<string, number>,
  ): void {
    for (const comment of read) {
      if (deletedBefore.get(comment.id) === this.#deleted.get(comment.id)) {
        this.#deleted.delete(comment.id);
      }
    }
    const comments = read.filter((comment) => !this.#deleted.has(comment.id));
    void this.#host.workspace.learnUsers(authorIds(comments));
    this.#comments = reread(this.#comments, comments, beforeIds);
    this.status = 'ready';
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
    const { beforeIds, deletedBefore } = this.snapshotRead();
    if (first || this.status === 'failed') this.status = 'loading';
    try {
      const read = await this.#host.api.list();
      if (this.#closed || reading !== this.#readings) return;
      this.#applyRead(read, beforeIds, deletedBefore);
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

  async commentVersions(commentId: string): Promise<CommentVersion[]> {
    if (!this.#host.api.versions) return [];
    const list = await this.#host.api.versions(commentId);
    await this.#host.workspace.learnUsers(
      list.map((version) => version.writtenBy).filter((id) => id !== ''),
    );
    return list;
  }
}
