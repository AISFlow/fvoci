import type { Notification } from '$lib/state/notification-types.ts';

/** What a line of the notifications list does to its notification, by its menu, a swipe or a key. */
export interface RowActions {
  /** Marks it read, or unread again where it was read. */
  readonly toggleRead: (notification: Notification) => void;
  /** Puts it away, or back into the inbox where it was put away. */
  readonly toggleArchive: (notification: Notification) => void;
}
