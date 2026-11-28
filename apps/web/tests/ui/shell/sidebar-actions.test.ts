/**
 * The menu of a board, a wiki or a diary of the sidebar: a diary's has no
 * copy and no template (a board's alone has that), and says what it is.
 */
import { describe, expect, it } from 'vitest';

import { containerActions, type SidebarMenus } from '$lib/features/shell/sidebar-actions.ts';
import { menuNamed, namedAs, shownTitle } from '$lib/features/shell/shown-title.ts';
import type { Container, ContainerKind } from '$lib/logic/wire.ts';
import type { ReaderStars } from '$lib/components/favorite-toggle/reader-stars.ts';
import type { Category } from '$lib/state/index.ts';

import { setup } from '../../state/support.ts';

const home = { id: 'home', type: 'system', name: '' } as Category;

function menuOf(kind: ContainerKind, stars?: ReaderStars): string[] {
  const { workspace } = setup();
  const menus: SidebarMenus = {
    stars,
    workspace,
    guest: false,
    containerId: undefined,
    canDeleteOpenContainer: false,
    onNewCategory: () => undefined,
    onRenameCategory: () => undefined,
    onDeleteCategory: () => undefined,
    onDeleteContainer: () => undefined,
    onTemplateFromBoard: () => undefined,
    onDuplicateContainer: () => undefined,
  };
  const container = { id: 'c', kind, title: '' } as Container;
  return containerActions(menus, container, home).map((action) => `${action.id}: ${action.label}`);
}

describe('the menu of a container of the sidebar', () => {
  it('stars a board, a wiki or a course first, once the stars are read, and never a diary', () => {
    const starred = new Set<string>();
    const stars: ReaderStars = {
      scope: 'w/me',
      has: (id) => starred.has(id),
      read: () => Promise.resolve(),
      toggle: () => Promise.resolve(),
    };
    expect(menuOf('board', stars)[0]).toBe('favorite: Add to favorites');
    starred.add('c');
    expect(menuOf('wiki', stars)[0]).toBe('favorite: Remove from favorites');
    expect(menuOf('course', stars)[0]).toBe('favorite: Remove from favorites');
    expect(menuOf('diary', stars).some((line) => line.startsWith('favorite'))).toBe(false);
    // Not read yet, or kept nowhere (a public link): no line.
    const unread: ReaderStars = { ...stars, has: () => undefined };
    expect(menuOf('board', unread)[0]).toBe('export: As a .fvoci archive');
  });

  it('of a board, a wiki and a diary has what each is, in its words', () => {
    expect(menuOf('board').map((line) => line.split(':')[0])).toEqual([
      'export',
      'export-markdown',
      'duplicate',
      'template-from-board',
      'hide',
      'delete',
    ]);
    expect(menuOf('wiki').map((line) => line.split(':')[0])).toEqual([
      'export',
      'export-markdown',
      'duplicate',
      'hide',
      'delete',
    ]);
    expect(menuOf('diary')).toEqual([
      'export: As a .fvoci archive',
      'export-markdown: As Markdown (.zip)',
      'hide: Hide diary',
      'delete: Delete diary',
    ]);
    expect(menuOf('course')).toEqual([
      'export: As a .fvoci archive',
      'export-markdown: As Markdown (.zip)',
      'duplicate: Duplicate course',
      'hide: Hide course',
      'delete: Delete course',
    ]);
  });

  it('names a diary and a course as their kinds, also without a title', () => {
    expect(shownTitle({ kind: 'diary', title: '' })).toBe('Diary');
    expect(shownTitle({ kind: 'diary', title: 'Days' })).toBe('Days');
    expect(namedAs('diary', 'Days')).toBe('Days, diary');
    expect(namedAs('board', 'Days')).toBeUndefined();
    expect(menuNamed('diary', 'Days')).toBe('Menu of the diary Days');
    expect(shownTitle({ kind: 'course', title: '' })).toBe('(Untitled course)');
    expect(shownTitle({ kind: 'course', title: 'Onboarding' })).toBe('Onboarding');
    expect(namedAs('course', 'Onboarding')).toBe('Onboarding, course');
    expect(menuNamed('course', 'Onboarding')).toBe('Menu of the course Onboarding');
  });
});
