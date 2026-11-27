/**
 * The lines of the two menus of the table: that of a column's heading and
 * that of a row. What a line does is the caller's; this says which lines
 * there are, in what order, and what they are called.
 */
import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
import CopyIcon from '@lucide/svelte/icons/copy';
import EyeOffIcon from '@lucide/svelte/icons/eye-off';
import FoldHorizontalIcon from '@lucide/svelte/icons/fold-horizontal';
import LinkIcon from '@lucide/svelte/icons/link';
import Maximize2Icon from '@lucide/svelte/icons/maximize-2';
import Trash2Icon from '@lucide/svelte/icons/trash-2';
import UnfoldHorizontalIcon from '@lucide/svelte/icons/unfold-horizontal';

import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';

import { TABLE_SETTINGS } from './columns.ts';

export interface HeaderMenu {
  /** How the view is sorted by this column, if it is its first sort. */
  sorted: 'ascending' | 'descending' | undefined;
  /**
   * Where the column stands among the property columns. Undefined for the
   * title column, which can be neither hidden nor moved.
   */
  place: { at: number; count: number } | undefined;
  /** The stored width, where the limits are known from it. */
  width: number | undefined;
  onSort: (reversed: boolean) => void;
  onHide: () => void;
  onMove: (by: -1 | 1) => void;
  /** Wider or narrower by so many pixels. */
  onResize: (delta: number) => void;
}

/**
 * Sorting, hiding, a step to the left or right, wider and narrower. The last
 * four are what a mouse does by dragging the heading and its edge; on a
 * touch screen the menu is the way.
 */
export function headerMenu(menu: HeaderMenu): MenuAction[] {
  const actions: MenuAction[] = [
    {
      id: 'sortAscending',
      label: m['TableHeaderMenu.sort-ascending'](),
      icon: ArrowUpIcon,
      checked: menu.sorted === 'ascending',
      run: () => {
        menu.onSort(false);
      },
    },
    {
      id: 'sortDescending',
      label: m['TableHeaderMenu.sort-descending'](),
      icon: ArrowDownIcon,
      checked: menu.sorted === 'descending',
      run: () => {
        menu.onSort(true);
      },
    },
  ];
  const place = menu.place;
  if (place) {
    actions.push(
      {
        id: 'hide',
        label: m['TableHeaderMenu.hide'](),
        icon: EyeOffIcon,
        separated: true,
        run: menu.onHide,
      },
      {
        id: 'moveLeft',
        label: m.table_column_move_left(),
        icon: ArrowLeftIcon,
        disabled: place.at <= 0,
        run: () => {
          menu.onMove(-1);
        },
      },
      {
        id: 'moveRight',
        label: m.table_column_move_right(),
        icon: ArrowRightIcon,
        disabled: place.at >= place.count - 1,
        run: () => {
          menu.onMove(1);
        },
      },
    );
  }
  actions.push(
    {
      id: 'wider',
      label: m.table_column_wider(),
      icon: UnfoldHorizontalIcon,
      separated: true,
      disabled: menu.width !== undefined && menu.width >= TABLE_SETTINGS.maxColumnWidth,
      run: () => {
        menu.onResize(TABLE_SETTINGS.menuStep);
      },
    },
    {
      id: 'narrower',
      label: m.table_column_narrower(),
      icon: FoldHorizontalIcon,
      disabled: menu.width !== undefined && menu.width <= TABLE_SETTINGS.minColumnWidth,
      run: () => {
        menu.onResize(-TABLE_SETTINGS.menuStep);
      },
    },
  );
  return actions;
}

export interface RowMenu {
  /** The reader may change cards: without it only opening and the link are offered. */
  canEdit: boolean;
  onOpen: () => void;
  onDuplicate: () => void;
  onCopyLink: () => void;
  /** No question is asked first: deleting a card can be undone. */
  onDelete: () => void;
}

/**
 * Opening comes first: where the device cannot hover, the row shows this
 * menu's button alone, and the menu is the way to open the card.
 */
export function rowMenu(menu: RowMenu): MenuAction[] {
  const actions: MenuAction[] = [
    { id: 'open', label: m['TableRow.open'](), icon: Maximize2Icon, run: menu.onOpen },
  ];
  if (menu.canEdit) {
    actions.push({
      id: 'duplicate',
      label: m['CardActionsMenu.duplicate'](),
      icon: CopyIcon,
      run: menu.onDuplicate,
    });
  }
  actions.push({
    id: 'copyLink',
    label: m['CardActionsMenu.copyLink'](),
    icon: LinkIcon,
    run: menu.onCopyLink,
  });
  if (menu.canEdit) {
    actions.push({
      id: 'delete',
      label: m['CardActionsMenu.delete'](),
      icon: Trash2Icon,
      destructive: true,
      separated: true,
      run: menu.onDelete,
    });
  }
  return actions;
}
