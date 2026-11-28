/** The title a board, wiki, diary or course is shown under: its own, or the words for one without. */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { Container, ContainerKind } from '$lib/logic/wire.ts';
import type { Favorite } from '$lib/state/index.ts';

export function shownTitle(container: Pick<Container, 'title' | 'kind'>): string {
  if (container.title) return container.title;
  if (container.kind === 'diary') return m.diary_untitled();
  if (container.kind === 'course') return m.course_untitled();
  return container.kind === 'wiki' ? m.wiki_untitled() : m['Sidebar.untitled-board']();
}

/** The title a star is shown under: what it is on's, or the words for one without. */
export function favoriteTitle(
  favorite: Pick<Favorite, 'kind' | 'title' | 'containerKind'>,
): string {
  if (favorite.title) return favorite.title;
  if (favorite.kind === 'card') return m['KanbanCard.untitled']();
  if (favorite.kind === 'page') return m.wiki_page_untitled();
  if (favorite.kind === 'unit') return m.course_unit_untitled();
  return shownTitle({ title: '', kind: favorite.containerKind });
}

/**
 * What a screen reader says of the link to a wiki, diary or course: its title
 * and what it is. A board's link says its title alone (none here).
 */
export function namedAs(kind: ContainerKind | undefined, title: string): string | undefined {
  if (kind === 'wiki') return m.shell_wiki_named({ title });
  if (kind === 'diary') return m.shell_diary_named({ title });
  if (kind === 'course') return m.shell_course_named({ title });
  return undefined;
}

/** The name of the menu of a container in the sidebar. */
export function menuNamed(kind: ContainerKind | undefined, name: string): string {
  if (kind === 'wiki') return m.shell_wiki_menu({ name });
  if (kind === 'diary') return m.shell_diary_menu({ name });
  if (kind === 'course') return m.shell_course_menu({ name });
  return m.shell_board_menu({ name });
}
