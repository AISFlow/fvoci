/**
 * One open unit of a course: its body, edited together in its room, and its
 * comment thread (`CommentThreadStore`).
 */
import { sameDocNode } from '../editor/same-doc.ts';
import type { DocNode } from '../editor/schema.ts';

import {
  isBackendError,
  isOwnSpace,
  type BodyDocState,
  type Courses,
  type CourseUnit,
  type CourseUnitDetail,
} from './backend.ts';
import { BodyUploads } from './body-uploads.svelte.ts';
import { CollaborativeBodyState } from './collab-body.svelte.ts';
import { CommentThreadStore } from './comment-thread.svelte.ts';
import type { CourseStore } from './course.svelte.ts';
import type { OpenHierarchyNode } from './hierarchy-writes.ts';
import type { WithReactions } from './reaction-types.ts';
import { mentionedIn } from './user-names.ts';
import type { LoadStatus } from './workspace.svelte.ts';

export class UnitStore implements OpenHierarchyNode {
  readonly course: CourseStore;
  readonly unitId: string;

  status = $state<LoadStatus>('idle');
  missing = $state(false);
  removed = $state.raw<{ by: 'others' | 'reader'; title: string } | undefined>(undefined);
  body = $state.raw<DocNode | null>(null);
  watching = $state<boolean | undefined>(undefined);
  watchingBusy = $state(false);

  #readAt = 0;
  #closed = false;
  #speculative: boolean;
  #news: CourseUnit | undefined;
  #readings = 0;
  #watchGen = 0;
  #shown = false;

  readonly comments: CommentThreadStore;
  readonly #collab: CollaborativeBodyState;
  readonly #bodyUploads: BodyUploads;

  constructor(course: CourseStore, unitId: string, options: { speculative?: boolean } = {}) {
    this.course = course;
    this.unitId = unitId;
    this.#speculative = Boolean(options.speculative);
    this.#bodyUploads = new BodyUploads(() => this.course.workspace);
    this.#collab = new CollaborativeBodyState({
      workspace: course.workspace,
      probeStillThere: () => this.#read(),
      onGone: () => {
        this.gone();
      },
      onDocReleased: () => {
        this.#catchUp();
      },
    });
    this.comments = new CommentThreadStore({
      workspace: course.workspace,
      parentId: unitId,
      containerId: course.courseId,
      role: () => this.course.role,
      joinable: () => this.course.joinable,
      isSpeculative: () => this.#speculative,
      onCommented: () => {
        if (this.canWatch) {
          this.#watchGen += 1;
          this.watching = true;
          this.course.noteWatching(this.unitId, true);
        }
      },
      api: {
        list: () => this.#courses.comments(this.#courseId, this.unitId),
        create: (body) => this.#courses.createComment(this.#courseId, this.unitId, body),
        patch: (commentId, write) =>
          this.#courses.patchComment(this.#courseId, this.unitId, commentId, write),
        remove: (commentId) => this.#courses.deleteComment(this.#courseId, this.unitId, commentId),
        restore: (commentId) =>
          this.#courses.restoreComment(this.#courseId, this.unitId, commentId),
        versions: (commentId) =>
          this.#courses.commentVersions(this.#courseId, this.unitId, commentId),
      },
    });
  }

  get id(): string {
    return this.unitId;
  }

  readonly fallbackTitle = '';

  get room(): BodyDocState | undefined {
    return this.#collab.room;
  }

  get lost(): readonly string[] {
    return this.#collab.lost;
  }

  get canWatch(): boolean {
    const { info, me } = this.course.workspace;
    return Boolean(info?.id && me?.id && !isOwnSpace(info));
  }

  get #courses(): Courses {
    const courses = this.course.workspace.backend.course;
    if (!courses) throw new Error('this backend has no courses');
    return courses;
  }

  get #courseId(): string {
    return this.course.courseId;
  }

  #read(): Promise<WithReactions<CourseUnitDetail>> {
    const courses = this.course.workspace.backend.course;
    if (!courses) return Promise.reject(new Error('this backend has no courses'));
    return courses.unit(this.#courseId, this.unitId);
  }

  async load(): Promise<void> {
    this.status = 'loading';
    await Promise.all([this.#readBody(), this.comments.read(), this.#readWatch()]);
  }

  async refresh(): Promise<void> {
    if (this.status !== 'ready') return;
    await Promise.all([
      this.#busy() ? undefined : this.#rereadBody(),
      this.comments.reread(),
      this.#readWatch(),
    ]);
  }

  async #readWatch(): Promise<void> {
    if (!this.canWatch) return;
    this.#watchGen += 1;
    const gen = this.#watchGen;
    const isWatching = await this.course.isWatchingUnit(this.unitId);
    if (!this.#closed && gen === this.#watchGen && isWatching !== undefined) {
      this.watching = isWatching;
    }
  }

  async setWatching(next: boolean): Promise<void> {
    if (!this.canWatch || this.watchingBusy) return;
    const courses = this.course.workspace.backend.course;
    const workspaceId = this.course.workspace.info?.id;
    const meId = this.course.workspace.me?.id;
    if (!courses || !workspaceId || !meId) return;
    const before = this.watching;
    this.#watchGen += 1;
    this.watching = next;
    this.course.noteWatching(this.unitId, next);
    this.watchingBusy = true;
    try {
      if (next) await courses.watchUnit(workspaceId, this.unitId, meId);
      else await courses.unwatchUnit(workspaceId, this.unitId, meId);
    } catch (error) {
      if (this.#closed) return;
      if (!next && isBackendError(error) && error.status === 404) {
        this.watching = false;
        this.course.noteWatching(this.unitId, false);
      } else {
        this.watching = before;
        if (before !== undefined) this.course.noteWatching(this.unitId, before);
        this.course.workspace.report('save', error);
      }
    } finally {
      this.watchingBusy = false;
    }
  }

  /** Applies an authoritative restored `CourseUnitDetail` immediately (W1.4). */
  applyRestored(detail: CourseUnitDetail): void {
    if (this.#closed || detail.kind === 'chapter') return;
    this.#readAt = detail.updatedAt;
    if (!sameDocNode(detail.body, this.body)) this.body = detail.body;
    void this.course.workspace.learnUsers(mentionedIn(detail.body));
    const { body: _body, createdBy: _cb, modifiedBy: _mb, ...summary } = detail;
    this.course.receiveUnit(summary);
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
    const mark = this.comments.reactionsMark();
    try {
      const { reactions, ...detail } = await this.#read();
      if (this.#closed || reading !== this.#readings) return;
      if (detail.kind === 'chapter') {
        if (this.#speculative) this.course.clearSpeculativeUnit(this.unitId);
        else this.gone();
        return;
      }
      // A reaction changes nothing of the unit: they are taken whatever its `updatedAt`.
      this.comments.takeReactions(reactions, mark);
      if (!first && detail.updatedAt <= this.#readAt) return;
      this.#readAt = detail.updatedAt;
      if (!sameDocNode(detail.body, this.body)) this.body = detail.body;
      void this.course.workspace.learnUsers(mentionedIn(detail.body));
      [this.status, this.missing] = ['ready', false];
      this.#shown = true;
      this.#speculative = false;
    } catch (error) {
      if (this.#closed || reading !== this.#readings) return;
      if (isBackendError(error) && error.status === 401) {
        if (first) this.status = 'failed';
        this.course.workspace.report('load', error);
      } else if (isBackendError(error) && (error.status === 404 || error.status === 403)) {
        this.gone();
      } else if (first) {
        this.status = 'failed';
        if (!this.#speculative) this.course.workspace.report('load', error);
      }
    }
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

  unitChanged(unit: CourseUnit): void {
    if (unit.id !== this.unitId || unit.updatedAt <= this.#readAt || this.missing) return;
    if (this.#busy()) {
      this.#news = unit;
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
    this.unitChanged(news);
  }

  openBody(): void {
    if (this.#closed || this.missing) return;
    this.#collab.open(
      this.course.workspace.backend,
      this.course.workspace.info?.id,
      this.#courseId,
      this.unitId,
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
      this.course.workspace.backend.upload(this.#courseId, file, options),
    );
  }
}
