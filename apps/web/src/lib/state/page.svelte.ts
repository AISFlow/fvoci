/**
 * One open page of a wiki: its body, edited together in its room, and its
 * comment thread (`CommentThreadStore`).
 */
import { sameDocNode } from '../editor/same-doc.ts';
import type { DocNode } from '../editor/schema.ts';
import {
  isBackendError,
  type BodyDocState,
  type WikiPage,
  type WikiPageDetail,
  type Wikis,
} from './backend.ts';
import { BodyUploads } from './body-uploads.svelte.ts';
import { CollaborativeBodyState } from './collab-body.svelte.ts';
import { CommentThreadStore } from './comment-thread.svelte.ts';
import type { OpenHierarchyNode } from './hierarchy-writes.ts';
import { mentionedIn } from './user-names.ts';
import type { WikiStore } from './wiki.svelte.ts';
import type { LoadStatus } from './workspace.svelte.ts';

export class PageStore implements OpenHierarchyNode {
  readonly wiki: WikiStore;
  readonly pageId: string;

  status = $state<LoadStatus>('idle');
  missing = $state(false);
  removed = $state.raw<{ by: 'others' | 'reader'; title: string } | undefined>(undefined);
  meta = $state.raw<WikiPage | undefined>(undefined);
  body = $state.raw<DocNode | null>(null);

  #readAt = 0;
  #closed = false;
  #speculative: boolean;
  #news: WikiPage | undefined;
  #readings = 0;
  #shown = false;

  readonly comments: CommentThreadStore;
  readonly #collab: CollaborativeBodyState;
  readonly #bodyUploads: BodyUploads;

  constructor(wiki: WikiStore, pageId: string, options: { speculative?: boolean } = {}) {
    this.wiki = wiki;
    this.pageId = pageId;
    this.#speculative = Boolean(options.speculative);
    this.#bodyUploads = new BodyUploads(() => this.wiki.workspace);
    this.#collab = new CollaborativeBodyState({
      workspace: wiki.workspace,
      probeStillThere: () => this.#read(),
      onGone: () => {
        this.gone();
      },
      onDocReleased: () => {
        this.#catchUp();
      },
    });
    this.comments = new CommentThreadStore({
      workspace: wiki.workspace,
      parentId: pageId,
      role: () => this.wiki.role,
      permissions: () => this.wiki.shownPermissions,
      joinable: () => this.wiki.joinable,
      isSpeculative: () => this.#speculative,
      api: {
        list: () => this.#wikis.comments(this.#wikiId, this.pageId),
        create: (body) =>
          this.wiki.writeJoined(() => this.#wikis.createComment(this.#wikiId, this.pageId, body)),
        patch: (commentId, write) =>
          this.wiki.writeJoined(() =>
            this.#wikis.patchComment(this.#wikiId, this.pageId, commentId, write),
          ),
        remove: (commentId) =>
          this.wiki.writeJoined(() =>
            this.#wikis.deleteComment(this.#wikiId, this.pageId, commentId),
          ),
        restore: (commentId) =>
          this.wiki.writeJoined(() =>
            this.#wikis.restoreComment(this.#wikiId, this.pageId, commentId),
          ),
        versions: (commentId) => this.#wikis.commentVersions(this.#wikiId, this.pageId, commentId),
      },
    });
  }

  get id(): string {
    return this.pageId;
  }

  get fallbackTitle(): string {
    return this.meta?.title ?? '';
  }

  get room(): BodyDocState | undefined {
    return this.#collab.room;
  }

  get lost(): readonly string[] {
    return this.#collab.lost;
  }

  get #wikis(): Wikis {
    const wikis = this.wiki.workspace.backend.wiki;
    if (!wikis) throw new Error('this backend has no wikis');
    return wikis;
  }

  get #wikiId(): string {
    return this.wiki.wikiId;
  }

  #read(): Promise<WikiPageDetail> {
    const wikis = this.wiki.workspace.backend.wiki;
    if (!wikis) return Promise.reject(new Error('this backend has no wikis'));
    return wikis.page(this.#wikiId, this.pageId);
  }

  async load(): Promise<void> {
    this.status = 'loading';
    await Promise.all([this.#readBody(), this.comments.read()]);
  }

  async refresh(): Promise<void> {
    if (this.status !== 'ready') return;
    await Promise.all([this.#busy() ? undefined : this.#rereadBody(), this.comments.reread()]);
  }

  #applyDetail(detail: WikiPageDetail): void {
    this.#readAt = detail.updatedAt;
    this.meta = {
      id: detail.id,
      wikiId: detail.wikiId,
      parentId: detail.parentId,
      title: detail.title,
      icon: detail.icon,
      order: detail.order,
      createdAt: detail.createdAt,
      updatedAt: detail.updatedAt,
    };
    if (!sameDocNode(detail.body, this.body)) this.body = detail.body;
    void this.wiki.workspace.learnUsers(mentionedIn(detail.body));
  }

  /** Applies an authoritative restored `WikiPageDetail` immediately (W1.4). */
  applyRestored(detail: WikiPageDetail): void {
    if (this.#closed) return;
    this.#applyDetail(detail);
    this.wiki.receivePage(this.meta ?? detail);
  }

  /** The first reading of the body: a failure is told. */
  #readBody(): Promise<void> {
    return this.#fetchBody('first');
  }

  /** The body read again when it changed: a failure waits for the next change. */
  #rereadBody(): Promise<void> {
    return this.#fetchBody('again');
  }

  async #fetchBody(kind: 'first' | 'again'): Promise<void> {
    const first = kind === 'first';
    this.#readings += 1;
    const reading = this.#readings;
    try {
      const detail = await this.#read();
      if (this.#closed || reading !== this.#readings) return;
      if (!first && detail.updatedAt <= this.#readAt) return;
      this.#applyDetail(detail);
      [this.status, this.missing] = ['ready', false];
      this.#shown = true;
      this.#speculative = false;
      this.wiki.workspace.rememberWikiPage(this.#wikiId, this.pageId);
    } catch (error) {
      if (this.#closed || reading !== this.#readings) return;
      if (isBackendError(error) && error.status === 401) {
        this.wiki.workspace.forgetWikiPage(this.#wikiId, this.pageId);
        if (first) this.status = 'failed';
        this.wiki.workspace.report('load', error);
      } else if (isBackendError(error) && (error.status === 404 || error.status === 403)) {
        this.wiki.workspace.forgetWikiPage(this.#wikiId, this.pageId);
        if (this.#isSpeculative()) this.wiki.speculativeFailed(this.pageId);
        else this.gone();
      } else if (first) {
        this.status = 'failed';
        if (this.#isSpeculative()) this.wiki.speculativeFailed(this.pageId);
        else this.wiki.workspace.report('load', error);
      }
    }
  }

  #isSpeculative(): boolean {
    return this.#speculative && this.wiki.speculativePageId === this.pageId;
  }

  close(): void {
    this.#closed = true;
    this.comments.close();
    this.closeBody();
    this.#bodyUploads.giveUp();
  }

  gone(by?: 'others' | 'reader', title = ''): void {
    if (by && this.#shown && !this.removed) this.removed = { by, title };
    this.missing = true;
    this.status = 'failed';
    this.closeBody();
  }

  pageChanged(page: WikiPage): void {
    if (page.id !== this.pageId || this.missing) return;
    if (!this.meta || page.updatedAt >= this.meta.updatedAt) this.meta = page;
    if (page.updatedAt <= this.#readAt) return;
    if (this.#busy()) {
      this.#news = page;
      return;
    }
    void this.#rereadBody();
  }

  #busy(): boolean {
    return this.#closed || this.#collab.hasDoc;
  }

  #catchUp(): void {
    const news = this.#news;
    if (!news || this.#busy()) return;
    this.#news = undefined;
    this.pageChanged(news);
  }

  // --------------------------------------------------------------- body

  openBody(): void {
    if (this.#closed || this.missing) return;
    this.#collab.open(
      this.wiki.workspace.backend,
      this.wiki.workspace.info?.id,
      this.#wikiId,
      this.pageId,
    );
  }

  closeBody(): void {
    this.#collab.close();
  }

  retryBody(): void {
    this.#collab.retry();
  }

  settleBody(): Promise<void> {
    return this.#collab.settle();
  }

  sayLost(why: string): void {
    this.#collab.sayLost(why);
  }

  dismissLost(): void {
    this.#collab.dismissLost();
  }

  uploadFile(file: File): Promise<string> {
    return this.#bodyUploads.run(file, (options) =>
      this.wiki.writeJoined(() => this.wiki.workspace.backend.upload(this.#wikiId, file, options)),
    );
  }
}
