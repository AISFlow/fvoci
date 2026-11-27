/** What a notification of the bell says (features/notifications/notification-words.ts). */
import { describe, expect, it } from 'vitest';

import { notificationText, unreadBadge } from '$lib/features/notifications/notification-words.ts';
import type { NotificationSubject } from '$lib/state/notification-types.ts';

const subject = (more: Partial<NotificationSubject>): NotificationSubject => ({
  kind: 'comment',
  actorId: 'u1',
  times: 1,
  people: 1,
  workspaceId: 'w',
  containerId: 'b',
  containerKind: 'board',
  blockId: 'c',
  blockKind: 'card',
  title: 'Plan',
  commentId: '',
  ...more,
});

describe('what a notification says', () => {
  it('names who did it, what, how many times, and to what', () => {
    expect(notificationText(subject({}), 'Ada')).toBe('Ada commented on Plan');
    expect(notificationText(subject({ times: 3 }), 'Ada')).toBe('Ada commented 3 times on Plan');
    expect(notificationText(subject({ kind: 'mention', times: 2 }), 'Ada')).toBe(
      'Ada mentioned you 2 times in Plan',
    );
    expect(notificationText(subject({ kind: 'assignment' }), 'Ada')).toBe('Ada gave you Plan');
    expect(
      notificationText(subject({ kind: 'meeting_upcoming', title: 'Weekly Sync' }), undefined),
    ).toBe('Upcoming study meeting: Weekly Sync');
  });

  it('counts the others who did it, and says so of whom it cannot name', () => {
    expect(notificationText(subject({ people: 2, times: 2 }), 'Ada')).toBe(
      'Ada and 1 other commented 2 times on Plan',
    );
    expect(notificationText(subject({ people: 4, times: 5, title: ' ' }), undefined)).toBe(
      'Somebody and 3 more commented 5 times on Untitled',
    );
  });

  it('says only that there was something, of what is no longer read', () => {
    expect(notificationText(null, 'Ada')).toBe('Something you no longer see');
  });

  it('counts the unread up to 99', () => {
    expect([unreadBadge(1), unreadBadge(99), unreadBadge(100)]).toEqual(['1', '99', '99+']);
  });
});
