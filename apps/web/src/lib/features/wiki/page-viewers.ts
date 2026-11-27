/**
 * Who else has a page of the wiki open now (the wiki's presence), and how
 * that is said: by the faces at the head of the page, and in the name of its
 * row of the tree, which a screen reader says in place of what is inside it.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { PresenceSource } from '$lib/state/index.ts';

/** The identifiers of the people other than the reader on the page, as the presence has them. */
export function viewersOf(source: PresenceSource, pageId: string): string[] {
  return source.presence.others
    .filter((person) => person.cardIds.includes(pageId))
    .map((person) => person.userId);
}

export function nameOf(source: PresenceSource, userId: string): string {
  return source.workspace.userName(userId) ?? m['centerPanel.unknown-user']();
}

/** "2 viewing this page/unit: …"; empty when nobody else is on it. */
export function viewersLabel(
  source: PresenceSource,
  userIds: readonly string[],
  of: 'page' | 'unit' = 'page',
): string {
  if (userIds.length === 0) return '';
  const names = userIds.map((userId) => nameOf(source, userId)).join(', ');
  return of === 'unit'
    ? m.course_unit_viewers({ count: userIds.length, names })
    : m.wiki_page_viewers({ count: userIds.length, names });
}
