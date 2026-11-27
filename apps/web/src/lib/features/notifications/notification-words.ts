/**
 * What a notification says: who did it (the latest of them, and how many
 * others), what, how many times, and to what. A notification whose subject
 * the reader no longer reads says only that there was something.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { NotificationSubject } from '$lib/state/notification-types.ts';

/**
 * Who did it: the latest of them by name, and how many others with them; one
 * whose name is not known is somebody. (In Korean the phrase carries its own
 * particle: one after a name put in would not fit every name.)
 */
export function whoDid(subject: NotificationSubject, name: string | undefined): string {
  const others = subject.people - 1;
  if (name === undefined) {
    return others <= 0
      ? m.notifications_somebody()
      : m.notifications_somebody_and_more({ others: String(others) });
  }
  if (others <= 0) return m.notifications_who_one({ name });
  if (others === 1) return m.notifications_who_and_one({ name });
  return m.notifications_who_and_more({ name, others: String(others) });
}

/** The line a notification says, `name` naming who did the latest of it. */
export function notificationText(
  subject: NotificationSubject | null,
  name: string | undefined,
): string {
  if (!subject) return m.notifications_gone();
  const who = whoDid(subject, name);
  const title = subject.title.trim() || m.notifications_untitled();
  const times = String(subject.times);
  switch (subject.kind) {
    case 'mention':
      return subject.times > 1
        ? m.notifications_mention_times({ who, title, times })
        : m.notifications_mention({ who, title });
    case 'assignment':
      return m.notifications_assignment({ who, title });
    case 'comment':
      return subject.times > 1
        ? m.notifications_comment_times({ who, title, times })
        : m.notifications_comment({ who, title });
    case 'review_due':
      return m.notifications_review_due({ title });
    case 'meeting_upcoming':
      return m.notifications_meeting_upcoming({ title });
    case 'due_soon':
      return m.notifications_due_soon({ title });
    case 'added_to_container':
      return m.notifications_added_to_container({ who, title });
  }
}

/** What the bell shows of the unread: up to 99, then `99+`. */
export function unreadBadge(unread: number): string {
  return unread > 99 ? '99+' : String(unread);
}

/** What the bell, or the line of a menu that stands for it, is called. */
export function bellLabel(unread: number): string {
  return unread > 0
    ? m.notifications_open_unread({ count: String(unread) })
    : m.notifications_open();
}
