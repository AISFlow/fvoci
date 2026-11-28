/**
 * The menus of the sidebar's categories and of its boards and wikis, apart
 * from the list (`SidebarList.svelte`), which is long.
 *
 * A wiki's menu is a board's without what is only a board's: making a
 * template of it. A diary's has neither that nor a copy: a diary is one of a
 * kind in its own space.
 */
import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
import CopyIcon from '@lucide/svelte/icons/copy';
import EyeOffIcon from '@lucide/svelte/icons/eye-off';
import FolderInputIcon from '@lucide/svelte/icons/folder-input';
import FolderPlusIcon from '@lucide/svelte/icons/folder-plus';
import LayoutTemplateIcon from '@lucide/svelte/icons/layout-template';
import PencilIcon from '@lucide/svelte/icons/pencil';
import Trash2Icon from '@lucide/svelte/icons/trash-2';

import { favoriteAction } from '$lib/components/favorite-toggle/favorite-action.ts';
import type { ReaderStars } from '$lib/components/favorite-toggle/reader-stars.ts';
import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import type { Container } from '$lib/logic/wire.ts';
import { copyPlaces, type Category, type Workspace } from '$lib/state/index.ts';

import { exportActions } from './container-export.ts';
import { categoryOrderAfterStep, moveToEnd } from './sidebar-logic.ts';

/**
 * The name a category is shown under. The default one is every reader's and
 * holds boards and wikis alike: it is named in the reader's language, not by
 * the name the server stored for it.
 */
export function categoryLabel(category: Pick<Category, 'type' | 'name'>): string {
  return category.type === 'system' ? m.shell_default_category() : category.name;
}

export type SidebarAsked =
  { kind: 'container'; container: Container } | { kind: 'category'; category: Category };

/** What the menus act with. */
export interface SidebarMenus {
  workspace: Workspace;
  /** A guest has the boards they were invited to as they are filed: theirs to save, not to arrange. */
  guest: boolean;
  /** The board or wiki on the page. */
  containerId: string | undefined;
  /** Whether the reader may delete the board or wiki on the page. Of the others the sidebar does not know. */
  canDeleteOpenContainer: boolean;
  /** Whether the currently open container is an open container the reader has not joined yet. */
  openContainerJoinable?: boolean | undefined;
  onNewCategory: () => void;
  onRenameCategory: (category: Category) => void;
  onDeleteCategory: (category: Category) => void;
  onDeleteContainer: (container: Container) => void;
  onTemplateFromBoard: (boardId: string) => void;
  onDuplicateContainer: (containerId: string) => void;
  onCopyToWorkspace?: ((container: Container) => void) | undefined;
  /** The reader's stars: a board, a wiki or a course is starred from its menu too. */
  stars?: ReaderStars | undefined;
}

function stepCategory(menus: SidebarMenus, category: Category, step: -1 | 1): void {
  const order = categoryOrderAfterStep(
    menus.workspace.sidebar.map((entry) => entry.category.id),
    category.id,
    step,
  );
  if (order) void menus.workspace.reorderCategories(order);
}

export function categoryActions(menus: SidebarMenus, category: Category): MenuAction[] {
  const { sidebar } = menus.workspace;
  // Its place among all the categories, not among those a search has left.
  const at = sidebar.findIndex((entry) => entry.category.id === category.id);
  const actions: MenuAction[] = [
    // The default category keeps its name: the server does not change it.
    ...(category.type === 'system'
      ? []
      : [
          {
            id: 'rename',
            label: m['SidebarCategories.CategoryMenu.Update'](),
            icon: PencilIcon,
            run: () => {
              menus.onRenameCategory(category);
            },
          },
        ]),
    {
      id: 'move-up',
      label: m.shell_category_move_up(),
      icon: ArrowUpIcon,
      disabled: at === 0,
      run: () => {
        stepCategory(menus, category, -1);
      },
    },
    {
      id: 'move-down',
      label: m.shell_category_move_down(),
      icon: ArrowDownIcon,
      disabled: at === sidebar.length - 1,
      run: () => {
        stepCategory(menus, category, 1);
      },
    },
    {
      id: 'new-category',
      label: m['SidebarCategories.CategoryMenu.CreateNew'](),
      icon: FolderPlusIcon,
      separated: true,
      run: menus.onNewCategory,
    },
  ];
  // The default category is the one every reader has; it cannot be deleted.
  if (category.type !== 'system') {
    actions.push({
      id: 'delete',
      label: m['SidebarCategories.CategoryMenu.Delete'](),
      icon: Trash2Icon,
      destructive: true,
      separated: true,
      run: () => {
        menus.onDeleteCategory(category);
      },
    });
  }
  return actions;
}

/** The lines that move a board, wiki or course from `home` to the end of another category. */
function movesOf(workspace: Workspace, container: Container, home: Category): MenuAction[] {
  return workspace.sidebar
    .filter((entry) => entry.category.id !== home.id)
    .map((entry): MenuAction => ({
      id: `move-to-${entry.category.id}`,
      label: m.shell_board_move_to({ category: categoryLabel(entry.category) }),
      icon: FolderInputIcon,
      run: () => {
        const move = moveToEnd(entry.category.id, container.id, workspace.sidebar);
        if (move) void workspace.moveContainer(move.containerId, move.categoryId, move.index);
      },
    }));
}

export function containerActions(
  menus: SidebarMenus,
  container: Container,
  home: Category,
): MenuAction[] {
  const { workspace } = menus;
  const kind = container.kind ?? 'board';
  const wiki = kind === 'wiki';
  const moves = movesOf(workspace, container, home);
  const unjoined = container.id === menus.containerId && menus.openContainerJoinable === true;
  // The star first, as at the head of the sidebar: set off from the moves after it.
  const star = kind === 'diary' ? undefined : favoriteAction(menus.stars, container.id);
  const starred = star ? [star] : [];
  const [firstMove, ...otherMoves] = moves;
  const moved = firstMove && star ? [{ ...firstMove, separated: true }, ...otherMoves] : moves;
  const before = starred.length + moves.length > 0;
  const saving = unjoined ? [] : exportActions(workspace, container, before);
  if (menus.guest) return [...starred, ...saving];
  const hasOtherWorkspace = copyPlaces(workspace.workspaces, container.workspaceId).length > 0;
  return [
    ...starred,
    ...moved,
    ...saving,
    ...(kind === 'diary' || unjoined
      ? []
      : [
          {
            id: 'duplicate',
            label:
              kind === 'course'
                ? m.shell_duplicate_course()
                : wiki
                  ? m.shell_duplicate_wiki()
                  : m['Sidebar.duplicate-board'](),
            icon: CopyIcon,
            run: () => {
              menus.onDuplicateContainer(container.id);
            },
          },
          ...(menus.onCopyToWorkspace && hasOtherWorkspace
            ? [
                {
                  id: 'copy-to-workspace',
                  label: m.shell_copy_to_workspace(),
                  icon: FolderInputIcon,
                  run: () => {
                    menus.onCopyToWorkspace?.(container);
                  },
                },
              ]
            : []),
        ]),
    ...(kind === 'board' && !unjoined
      ? [
          {
            id: 'template-from-board',
            label: m['Sidebar.template-from-board'](),
            icon: LayoutTemplateIcon,
            run: () => {
              menus.onTemplateFromBoard(container.id);
            },
          },
        ]
      : []),
    {
      id: 'hide',
      label: {
        board: m['HideBoard.MenuOption'],
        wiki: m.shell_hide_wiki,
        diary: m.shell_hide_diary,
        course: m.shell_hide_course,
      }[kind](),
      icon: EyeOffIcon,
      run: () => void workspace.hideContainer(container.id),
    },
    {
      id: 'delete',
      label: {
        board: m['Sidebar.delete-board'],
        wiki: m.shell_delete_wiki,
        diary: m.shell_delete_diary,
        course: m.shell_delete_course,
      }[kind](),
      icon: Trash2Icon,
      destructive: true,
      separated: true,
      // Of the board or wiki on the page the sidebar knows; of the others the server has the say.
      disabled: container.id === menus.containerId && !menus.canDeleteOpenContainer,
      run: () => {
        menus.onDeleteContainer(container);
      },
    },
  ];
}
