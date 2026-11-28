/**
 * What the reader was told, part of the data boundary (`backend.ts`): the
 * server's `getNotifications`, `getUnreadNotifications`, `readNotification`
 * and `readNotifications`, and the inbox's sorting: `unreadNotification`,
 * `archiveNotification`, `unarchiveNotification` and
 * `archiveReadNotifications`. What happens to one card or page, of one kind,
 * within an hour of the first of it is one notification that counts it
 * (`times`, `people`); more of it makes it unread again, puts it first, and
 * brings it back into the inbox if it was put away. One put away is read.
 * When what the reader was told changes, `watchSelf` hears `notifications`.
 */
import type { NotificationKind as NotifySettingKind } from '../logic/notify-settings.ts';
import type { ContainerKind } from '../logic/wire.ts';

import type { SearchKind } from './content-search.ts';
import type { Paging } from './trash-reads.ts';

/** What a notification tells of, including personal due-review, course meeting, and container-add notifications. */
export type NotificationKind =
  NotifySettingKind | 'review_due' | 'meeting_upcoming' | 'added_to_container';

/** What a notification is about, for a reader who reads it now. */
export interface NotificationSubject {
  kind: NotificationKind;
  /** Who did the latest of it; empty where nobody is known to have. */
  actorId: string;
  /** How many times it happened. */
  times: number;
  /** How many people did it, `actorId` among them. */
  people: number;
  workspaceId: string;
  /** The board the card is on, or the wiki the page is in. */
  containerId: string;
  containerKind: ContainerKind;
  /** The card or the page: what the reader opens. */
  blockId: string;
  blockKind: SearchKind;
  /** Its title as it is now. */
  title: string;
  /** The comment the latest of it happened in, where that is still there; else empty. */
  commentId: string;
  /**
   * The personal API token through which an agent of `actorId` did the latest of it;
   * `undefined` where they did it themselves.
   */
  agentTokenName?: string | undefined;
}

/** One thing the reader was told. */
export interface Notification {
  id: string;
  /** When the first of it happened. */
  createdAt: number;
  /** When the latest of it happened: the list is in this order, the newest first. */
  updatedAt: number;
  /** When the reader read it; 0 until then, and again once more of it happens. */
  readAt: number;
  /** When the reader put it away; 0 while it is in the inbox, and again once more of it happens. */
  archivedAt: number;
  /** `null` where the reader does not read that now: nothing of it is said. */
  subject: NotificationSubject | null;
}

/**
 * Which of the reader's notifications are listed: of the inbox, or of what was
 * put away (`archived`); only the unread; only those of one kind, where those
 * the reader reads no more (`subject` null) are left out, as they say nothing
 * of their kind.
 */
export interface NotificationFilter {
  unread?: boolean;
  kind?: 'mention' | 'assignment';
  archived?: boolean;
}

export interface Notifications {
  /** The reader's notifications that `filter` takes (the inbox), the newest first; 50 at a time unless `paging` says otherwise. */
  list(paging?: Paging, filter?: NotificationFilter): Promise<Notification[]>;
  /** How many the reader has not read. */
  unread(): Promise<number>;
  /** Marks one of the reader's as read; 404 for one that is not theirs, or none. */
  read(notificationId: string): Promise<void>;
  /** Marks as read all of the reader's whose latest happened by `through` (now when left out). */
  readAll(through?: number): Promise<void>;
  /** Marks one of the reader's unread again, back in the inbox; 404 as `read`. */
  markUnread(notificationId: string): Promise<void>;
  /** Puts one of the reader's away, read; 404 as `read`. */
  archive(notificationId: string): Promise<void>;
  /** Brings one of the reader's back into the inbox, read; 404 as `read`. */
  unarchive(notificationId: string): Promise<void>;
  /** Puts away all of the reader's that were read and are in the inbox. */
  archiveRead(): Promise<void>;
}

/** Where a backend keeps what the reader was told; none through a public link. */
export interface NotificationsOfBackend {
  readonly notifications: Notifications | undefined;
}
