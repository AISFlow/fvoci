/**
 * One open card: its body, comments and attachments.
 *
 * The title, icon and properties of the card live in `BoardStore.cards`
 * (`summary`) so the card and board never disagree. Collaborative body room
 * state is composed via `CollaborativeBodyState` and comment operations via
 * `CommentThreadStore`.
 */
import { sameDocNode } from '../editor/same-doc.ts';
import type { DocNode } from '../editor/schema.ts';
import {
  isBackendError,
  type Attachment,
  type BodyDocState,
  type CardDetail,
  type CardSummary,
  type Comment,
} from './backend.ts';
import { BodyUploads } from './body-uploads.svelte.ts';
import type { BoardStore } from './board.svelte.ts';
import type { CommentEdit } from './card-parts.ts';
import { CollaborativeBodyState, SAVING_SHOWN } from './collab-body.svelte.ts';
import { CommentThreadStore } from './comment-thread.svelte.ts';
import { byCreation } from './order.ts';
import { UploadKept, type Transfer } from './transfers.svelte.ts';
import { authorIds } from './user-names.ts';
import type { LoadStatus } from './workspace.svelte.ts';
import { reread, take } from './write.ts';

export { SAVING_SHOWN };

export class CardStore {
  readonly board: BoardStore;
  readonly cardId: string;

  status = $state<LoadStatus>('idle');
  #refreshing = 0;
  missing = $state(false);
  body = $state.raw<DocNode | null>(null);

  readonly uploads = $derived.by<Transfer[]>(() =>
    this.board.workspace.transfers.list.filter(
      (one) => one.card === this.cardId && !one.finished && one.failure !== 'kept',
    ),
  );

  #attachments = $state.raw<Attachment[]>([]);
  readonly attachments = $derived(byCreation(this.#attachments));
  readonly summary = $derived.by<CardSummary | undefined>(() => this.board.card(this.cardId));

  #readAt = 0;
  #closed = false;
  #news: CardSummary | undefined;
  readonly #readBoardAgain: () => void;
  readonly #collab: CollaborativeBodyState;
  readonly #commentThread: CommentThreadStore;
  readonly #bodyUploads: BodyUploads;

  constructor(board: BoardStore, cardId: string, readBoardAgain: () => void) {
    this.board = board;
    this.cardId = cardId;
    this.#readBoardAgain = readBoardAgain;
    this.#bodyUploads = new BodyUploads(() => this.board.workspace);
    this.#collab = new CollaborativeBodyState({
      workspace: board.workspace,
      trackSaving: true,
      skipProbeWhileLoading: () => this.status === 'loading',
      probeStillThere: () => this.#backend.card(this.#boardId, this.cardId),
      onGone: () => {
        this.missing = true;
        this.#readBoardAgain();
      },
      onDocReleased: () => {
        this.#catchUp();
      },
    });
    this.#commentThread = new CommentThreadStore({
      workspace: board.workspace,
      parentId: cardId,
      role: () => this.board.role,
      permissions: () => this.board.effectivePermissions,
      joinable: () => this.board.joinable,
      onDeleting: (commentId) => {
        this.board.deleting(commentId, this.cardId);
      },
      onConflict: () => this.refresh(),
      api: {
        list: () => this.#backend.comments(this.#boardId, this.cardId),
        create: (body) =>
          this.board.writeJoined(() =>
            this.#backend.createComment(this.#boardId, this.cardId, body),
          ),
        patch: (commentId, write) =>
          this.board.writeJoined(() =>
            this.#backend.patchComment(this.#boardId, commentId, write, this.cardId),
          ),
        remove: (commentId) =>
          this.board.writeJoined(() =>
            this.#backend.deleteComment(this.#boardId, this.cardId, commentId),
          ),
        restore: (commentId) =>
          this.board.writeJoined(() =>
            this.#backend.restoreComment(this.#boardId, this.cardId, commentId),
          ),
        versions: (commentId) =>
          this.#backend.commentVersions(this.#boardId, commentId, this.cardId),
      },
    });
  }

  get #backend(): BoardStore['workspace']['backend'] {
    return this.board.workspace.backend;
  }

  get #boardId(): string {
    return this.board.boardId;
  }

  get room(): BodyDocState | undefined {
    return this.#collab.room;
  }

  get lost(): readonly string[] {
    return this.#collab.lost;
  }

  get bodyEdited(): boolean {
    return this.#collab.bodyEdited;
  }

  get bodySaving(): boolean {
    return this.#collab.bodySaving;
  }

  get bodyPending(): boolean {
    return this.#collab.pending;
  }

  get comments(): Comment[] {
    return this.#commentThread.comments;
  }

  async load(): Promise<void> {
    this.status = 'loading';
    this.openBody();
    try {
      const [detail, comments, attachments] = await Promise.all([
        this.#backend.card(this.#boardId, this.cardId),
        this.#backend.comments(this.#boardId, this.cardId),
        this.#backend.attachments(this.#boardId, this.cardId),
      ]);
      if (this.#closed) return;
      this.#takeDetail(detail);
      void this.board.workspace.learnUsers(authorIds([detail, ...comments]));
      this.#commentThread.setInitial(comments);
      this.#attachments = attachments;
      this.status = 'ready';
      this.#collab.probeIfNotFound();
    } catch (error) {
      this.status = 'failed';
      this.closeBody();
      if (isBackendError(error) && error.status === 404) this.missing = true;
      else this.board.workspace.report('load', error);
    }
  }

  close(): void {
    this.#closed = true;
    this.#commentThread.close();
    this.closeBody();
    this.#bodyUploads.giveUp();
  }

  #takeDetail(detail: CardDetail): void {
    const { body, ...summary } = detail;
    this.#readAt = detail.updatedAt;
    this.missing = false;
    this.board.receiveCard(summary);
    if (!sameDocNode(body, this.body)) this.body = body;
  }

  // --------------------------------------------------------------- body

  openBody(): void {
    if (this.#closed || this.missing) return;
    this.#collab.open(this.#backend, this.board.workspace.info?.id, this.#boardId, this.cardId);
  }

  closeBody(): void {
    this.#collab.close();
  }

  settleBody(): Promise<void> {
    return this.#collab.settle();
  }

  retryBody(): void {
    this.#collab.retry();
  }

  sayLost(why: string): void {
    this.#collab.sayLost(why);
  }

  dismissLost(): void {
    this.#collab.dismissLost();
  }

  /** Applies an authoritative restored `CardDetail` immediately (W1.4). */
  applyRestored(detail: CardDetail): void {
    if (this.#closed) return;
    this.#takeDetail(detail);
  }

  cardChanged(card: CardSummary): void {
    const back = this.missing;
    if (card.id !== this.cardId || (!back && card.updatedAt <= this.#readAt)) return;
    if (this.#busy() && !back) {
      this.#news = card;
      return;
    }
    void this.#backend
      .card(this.#boardId, this.cardId)
      .then((detail) => {
        if (this.#busy() && !back) {
          this.#news = card;
          return;
        }
        if (!back && detail.updatedAt <= this.#readAt) return;
        this.#takeDetail(detail);
      })
      .catch(() => undefined);
  }

  #busy(): boolean {
    return this.#closed || this.#collab.hasDoc;
  }

  #catchUp(): void {
    const news = this.#news;
    if (!news || this.#busy()) return;
    this.#news = undefined;
    this.cardChanged(news);
  }

  // ------------------------------------------- comments and attachments

  receive(block: Comment | Attachment): void {
    if (block.parentId !== this.cardId) return;
    if (block.type === 'comment') {
      this.#commentThread.receive(block);
      return;
    }
    void this.board.workspace.learnUsers(authorIds([block]));
    this.#attachments = take(this.#attachments, block);
  }

  async refresh(): Promise<void> {
    if (this.status !== 'ready' || this.#closed) return;
    this.#refreshing += 1;
    const reading = this.#refreshing;
    const commentsBefore = this.#commentThread.snapshotRead();
    const attachmentsBefore = this.#attachments.map((attachment) => attachment.id);
    try {
      const [comments, attachments] = await Promise.all([
        this.#backend.comments(this.#boardId, this.cardId),
        this.#backend.attachments(this.#boardId, this.cardId),
      ]);
      if (this.#isClosed() || reading !== this.#refreshing) return;
      this.#commentThread.mergeRead(
        comments,
        commentsBefore.beforeIds,
        commentsBefore.deletedBefore,
      );
      this.#attachments = reread(this.#attachments, attachments, attachmentsBefore);
    } catch (error) {
      if (this.#isClosed() || reading !== this.#refreshing) return;
      if (isBackendError(error) && error.status === 404) this.missing = true;
    }
  }

  #isClosed(): boolean {
    return this.#closed;
  }

  forget(blockIds: readonly string[]): void {
    if (blockIds.includes(this.cardId)) this.missing = true;
    this.#commentThread.forget(blockIds);
    this.#attachments = this.#attachments.filter((attachment) => !blockIds.includes(attachment.id));
  }

  addComment(body: DocNode): Promise<boolean> {
    return this.#commentThread.addComment(body);
  }

  editComment(commentId: string, body: DocNode, readAt: number): Promise<CommentEdit> {
    return this.#commentThread.editComment(commentId, body, readAt);
  }

  comment(commentId: string): Comment | undefined {
    return this.#commentThread.comment(commentId);
  }

  deleteComment(commentId: string): Promise<void> {
    return this.#commentThread.deleteComment(commentId);
  }

  async deleteAttachment(attachmentId: string): Promise<void> {
    const child = this.#attachments.find((existing) => existing.id === attachmentId);
    try {
      await this.board.workspace.undo.perform(
        async () => {
          this.board.deleting(attachmentId);
          await this.board.writeJoined(() =>
            this.#backend.deleteBlock(this.#boardId, attachmentId),
          );
          this.forget([attachmentId]);
        },
        async () => {
          await this.board.writeJoined(() =>
            this.#backend.restoreBlock(this.#boardId, attachmentId),
          );
          if (child) this.receive(child);
        },
        'deleteAttachment',
      );
    } catch (error) {
      this.board.workspace.report('save', error);
    }
  }

  /** Attaches `file` to the card; where it is the reader's first write on an open board, once they have joined it. */
  async attach(file: File): Promise<void> {
    const board = this.board;
    const { workspace, boardId } = board;
    const { backend } = workspace;
    const cardId = this.cardId;
    const shown = (attachment: Attachment): void => {
      const open = workspace.board?.openCard;
      if (open?.cardId === cardId && open.board.boardId === boardId) open.receive(attachment);
    };
    await workspace.transfers.upload({
      file,
      card: cardId,
      retryable: true,
      async send(options) {
        const uploaded = await board.writeJoined(() => backend.upload(boardId, file, options));
        options.signal.throwIfAborted();
        const attachment = await backend.createAttachment(boardId, cardId, uploaded);
        if (!options.signal.aborted) {
          shown(attachment);
          return;
        }
        try {
          await backend.deleteBlock(boardId, attachment.id);
        } catch (error) {
          if (isBackendError(error) && error.status === 404) return;
          shown(attachment);
          throw new UploadKept(error);
        }
      },
    }).done;
  }

  uploadFile(file: File): Promise<string> {
    return this.#bodyUploads.run(file, (options) =>
      this.board.writeJoined(() => this.#backend.upload(this.#boardId, file, options)),
    );
  }

  get filesGoingUp(): boolean {
    return this.#bodyUploads.goingUp;
  }

  async fileBytes(fileId: string, signal?: AbortSignal): Promise<Blob | undefined> {
    try {
      return await this.#backend.fileBytes(this.#boardId, fileId, signal);
    } catch (error) {
      if (!signal?.aborted) this.board.workspace.report('load', error);
      return undefined;
    }
  }

  fileUrl(fileId: string): string | undefined {
    return this.#backend.fileUrl(this.#boardId, fileId);
  }
}
