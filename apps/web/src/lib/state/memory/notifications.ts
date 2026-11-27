/**
 * What the memory backend's people are told, as the server tells it of
 * comments: a new comment on a card or a page tells whoever watches it, but
 * its writer. Who watches is worked out from the world, not kept: who wrote
 * the card or the page, who commented on it, and whom a property of people
 * of the card holds. Each is told only of what they read, and only with
 * their switch of comments on (`notifyComments`).
 *
 * What happens to one card or page within an hour of the first of it joins
 * that notification, counted, by the latest, and unread again. Mentions and
 * cards given tell nothing here: the server's tests hold those.
 */
import { notifies } from '../../logic/notify-settings.ts';
import type { Paging } from '../trash-reads.ts';
import type { Notification, NotificationKind, Notifications } from '../notification-types.ts';

import { settingsOf } from './account.ts';
import { reads } from './container-access.ts';
import { fail } from './refuse.ts';
import type { StoredBlock, World } from './world.ts';

/** How long after the first of it more of the same joins a notification. */
const HOUR = 60 * 60 * 1000;

/** A notification as the world keeps it. */
export interface StoredNotification {
  id: string;
  recipientId: string;
  /** Defaults to `'comment'` when omitted. */
  kind?: NotificationKind;
  /** The card, page, unit, or meeting, and where it is. */
  blockId: string;
  containerId: string;
  /** The comment of the latest of it. */
  commentId: string;
  actorId: string;
  /** Who did it, each once. */
  doers: string[];
  times: number;
  createdAt: number;
  updatedAt: number;
  readAt: number;
}

/**
 * Delivers an upcoming-meeting notification in the shared workspace to each
 * active enrolled member of `courseId`, matching `App::notify_upcoming_meetings`.
 */
export function noteUpcomingMeeting(world: World, courseId: string, meetingId: string): void {
  const container = world.containers.get(courseId);
  const meeting = (world.courseMeetings.get(courseId) ?? []).find((one) => one.id === meetingId);
  if (!container || !meeting || container.workspaceId !== world.workspace.id) return;
  const now = world.now();
  const told: string[] = [];
  for (const member of world.members.get(courseId) ?? []) {
    if (!reads(world, member.userId, courseId)) continue;
    const theirs = world.notifications.get(member.userId) ?? [];
    theirs.push({
      id: world.newId(),
      recipientId: member.userId,
      kind: 'meeting_upcoming',
      blockId: meeting.id,
      containerId: courseId,
      commentId: '',
      actorId: '',
      doers: [],
      times: 1,
      createdAt: now,
      updatedAt: now,
      readAt: 0,
    });
    world.notifications.set(member.userId, theirs);
    told.push(member.userId);
  }
  meeting.notifiedAt = now;
  world.tellUsers(told, { kind: 'notifications' });
}

/**
 * Tells the watchers of what `comment` is on of it, the comment's writer
 * aside, and tells their pages that what they were told changed.
 */
export function noteComment(world: World, comment: StoredBlock): void {
  const on = world.blocks.get(comment.parentId);
  const container = world.containers.get(comment.containerId);
  if (
    (on?.type !== 'card' && on?.type !== 'page' && on?.type !== 'unit') ||
    !container ||
    container.isTemplate ||
    container.workspaceId !== world.workspace.id
  ) {
    return;
  }
  const now = comment.createdAt;
  const told: string[] = [];
  for (const recipientId of watchersOf(world, on)) {
    if (recipientId === comment.createdBy || !reads(world, recipientId, container.id)) continue;
    if (!notifies(settingsOf(world, recipientId), 'comment')) continue;
    const theirs = world.notifications.get(recipientId) ?? [];
    const same = theirs.find((one) => one.blockId === on.id && one.createdAt > now - HOUR);
    if (same) {
      same.times += 1;
      same.actorId = comment.createdBy;
      same.commentId = comment.id;
      same.containerId = container.id;
      if (!same.doers.includes(comment.createdBy)) same.doers.push(comment.createdBy);
      same.updatedAt = now;
      same.readAt = 0;
    } else {
      theirs.push({
        id: world.newId(),
        recipientId,
        blockId: on.id,
        containerId: container.id,
        commentId: comment.id,
        actorId: comment.createdBy,
        doers: [comment.createdBy],
        times: 1,
        createdAt: now,
        updatedAt: now,
        readAt: 0,
      });
    }
    world.notifications.set(recipientId, theirs);
    told.push(recipientId);
  }
  world.tellUsers(told, { kind: 'notifications' });
}

/** Who watches a card or a page: who wrote it, who commented on it, whom its people properties hold. */
function watchersOf(world: World, block: StoredBlock): Set<string> {
  const watchers = new Set<string>([block.createdBy]);
  for (const child of world.childrenOf(block.id)) {
    if (child.type === 'comment') watchers.add(child.createdBy);
  }
  const properties = world.containers.get(block.containerId)?.cardProperties ?? [];
  const values = (block.fields as { properties?: Record<string, unknown> }).properties ?? {};
  for (const { id, type } of properties) {
    if (type !== 'person' && type !== 'multiPerson') continue;
    const value = values[id];
    for (const one of Array.isArray(value) ? value : [value]) {
      if (typeof one === 'string' && one !== '') watchers.add(one);
    }
  }
  for (const [userId, byBlock] of world.unitWatchers) {
    const explicit = byBlock.get(block.id);
    if (explicit === true) watchers.add(userId);
    else if (explicit === false) watchers.delete(userId);
  }
  return watchers;
}

/**
 * Adds a personal `review_due` reminder notification for a unit, matching
 * `Store::notify_due_review` on the server.
 */
export function noteReviewDue(
  world: World,
  recipientId: string,
  courseId: string,
  unitId: string,
): StoredNotification | undefined {
  const block = world.blocks.get(unitId);
  const container = world.containers.get(courseId);
  if (block?.type !== 'unit' || block.containerId !== courseId || container?.kind !== 'course') {
    return undefined;
  }
  const now = world.now();
  const theirs = world.notifications.get(recipientId) ?? [];
  const created: StoredNotification = {
    id: world.newId(),
    recipientId,
    kind: 'review_due',
    blockId: unitId,
    containerId: courseId,
    commentId: '',
    actorId: '',
    doers: [],
    times: 1,
    createdAt: now,
    updatedAt: now,
    readAt: 0,
  };
  theirs.push(created);
  world.notifications.set(recipientId, theirs);
  world.tellUsers([recipientId], { kind: 'notifications' });
  return created;
}

/** The reader's notifications in the world, as the server answers them. */
export class MemoryNotifications implements Notifications {
  readonly #of: { readonly world: World; readonly userId: string };
  readonly #enter: () => Promise<void>;

  /**
   * Of the reader and the world of `of` (the backend: read when asked, not
   * before). `enter` waits as every call of the backend does, and fails where
   * it was told to.
   */
  constructor(of: { readonly world: World; readonly userId: string }, enter: () => Promise<void>) {
    this.#of = of;
    this.#enter = enter;
  }

  get #world(): World {
    return this.#of.world;
  }

  get #userId(): string {
    return this.#of.userId;
  }

  get #theirs(): StoredNotification[] {
    return this.#world.notifications.get(this.#userId) ?? [];
  }

  async list(paging: Paging = { page: 0, perPage: 50 }): Promise<Notification[]> {
    await this.#enter();
    const newest = [...this.#theirs].sort((a, b) => b.updatedAt - a.updatedAt);
    const start = paging.page * paging.perPage;
    return newest.slice(start, start + paging.perPage).map((one) => this.#shown(one));
  }

  async unread(): Promise<number> {
    await this.#enter();
    return this.#theirs.filter((one) => one.readAt === 0).length;
  }

  async read(notificationId: string): Promise<void> {
    await this.#enter();
    const one = this.#theirs.find((each) => each.id === notificationId);
    if (!one) fail(404, 'no such notification');
    if (one.readAt !== 0) return;
    one.readAt = this.#world.now();
    this.#world.tellUsers([this.#userId], { kind: 'notifications' });
  }

  async readAll(through?: number): Promise<void> {
    await this.#enter();
    const now = this.#world.now();
    const read = this.#theirs.filter(
      (one) => one.readAt === 0 && one.updatedAt <= (through ?? now),
    );
    for (const one of read) one.readAt = now;
    if (read.length > 0) this.#world.tellUsers([this.#userId], { kind: 'notifications' });
  }

  /** A notification as its reader is told it now: nothing of what they do not read. */
  #shown(one: StoredNotification): Notification {
    const world = this.#world;
    const { id, createdAt, updatedAt, readAt } = one;
    if (one.kind === 'meeting_upcoming') {
      const container = world.containers.get(one.containerId);
      const meeting = (world.courseMeetings.get(one.containerId) ?? []).find(
        (each) => each.id === one.blockId,
      );
      if (!container || !meeting || !reads(world, this.#userId, container.id)) {
        return { id, createdAt, updatedAt, readAt, subject: null };
      }
      return {
        id,
        createdAt,
        updatedAt,
        readAt,
        subject: {
          kind: 'meeting_upcoming',
          actorId: '',
          times: 1,
          people: 0,
          workspaceId: container.workspaceId,
          containerId: container.id,
          containerKind: 'course',
          blockId: meeting.id,
          blockKind: 'unit',
          title: meeting.title,
          commentId: '',
        },
      };
    }
    if (one.kind === 'added_to_container') {
      const container = world.containers.get(one.containerId);
      if (!container || !reads(world, this.#userId, container.id)) {
        return { id, createdAt, updatedAt, readAt, subject: null };
      }
      return {
        id,
        createdAt,
        updatedAt,
        readAt,
        subject: {
          kind: 'added_to_container',
          actorId: one.actorId,
          times: one.times,
          people: one.doers.length,
          workspaceId: container.workspaceId,
          containerId: container.id,
          containerKind: container.kind ?? 'board',
          blockId: '',
          blockKind:
            container.kind === 'wiki' ? 'page' : container.kind === 'course' ? 'unit' : 'card',
          title: container.title,
          commentId: '',
        },
      };
    }
    const block = world.blocks.get(one.blockId);
    const container = world.containers.get(block?.containerId ?? '');
    if (!block || !container || !reads(world, this.#userId, container.id)) {
      return { id, createdAt, updatedAt, readAt, subject: null };
    }
    const comment = one.commentId ? world.blocks.get(one.commentId) : undefined;
    const kind = one.kind ?? 'comment';
    const subject = {
      kind,
      actorId: one.actorId,
      times: one.times,
      people: one.doers.length,
      workspaceId: container.workspaceId,
      containerId: container.id,
      containerKind: container.kind ?? 'board',
      blockId: block.id,
      blockKind:
        block.type === 'page'
          ? ('page' as const)
          : block.type === 'unit'
            ? ('unit' as const)
            : ('card' as const),
      title: block.title,
      commentId:
        kind === 'review_due' || kind === 'due_soon'
          ? ''
          : comment?.parentId === block.id
            ? comment.id
            : '',
    };
    return { id, createdAt, updatedAt, readAt, subject };
  }
}
