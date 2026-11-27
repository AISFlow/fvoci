/**
 * Labels and row action menu builder for `PageTree` and `MovePageDialog`,
 * shared between wikis and courses so `PageTree.svelte` stays concise.
 */
import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
import CopyIcon from '@lucide/svelte/icons/copy';
import FilePlusIcon from '@lucide/svelte/icons/file-plus';
import FolderInputIcon from '@lucide/svelte/icons/folder-input';
import ListPlusIcon from '@lucide/svelte/icons/list-plus';
import PencilIcon from '@lucide/svelte/icons/pencil';
import Trash2Icon from '@lucide/svelte/icons/trash-2';

import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import type { MoveStep, TreeRules } from '$lib/logic/page-moves.ts';
import type { PageRow } from '$lib/logic/page-tree.ts';
import type { PageDraft, TreeItem } from '$lib/state/index.ts';

/** Wordings for `PageTree`: defaults to wiki pages, overridden for courses. */
export interface TreeLabels {
  heading: () => string;
  untitled: (item: TreeItem) => string;
  menuLabel: (item: TreeItem, title: string) => string;
  addChild: () => string;
  addAfter: () => string;
  rename?: () => string;
  move: () => string;
  deleteItem: (item: TreeItem) => string;
  viewersOf?: 'page' | 'unit';
}

export const WIKI_TREE_LABELS: TreeLabels = {
  heading: () => m.wiki_pages(),
  untitled: () => m.wiki_page_untitled(),
  menuLabel: (_item, title) => m.wiki_page_menu({ title }),
  addChild: () => m.wiki_add_child(),
  addAfter: () => m.wiki_add_after(),
  move: () => m.wiki_move_page(),
  deleteItem: () => m.wiki_delete_page(),
  viewersOf: 'page',
};

export const COURSE_TREE_LABELS: TreeLabels = {
  heading: () => m.course_units(),
  untitled: (item) =>
    item.kind === 'chapter' ? m.course_chapter_untitled() : m.course_unit_untitled(),
  menuLabel: (item, title) =>
    item.kind === 'chapter' ? m.course_chapter_menu({ title }) : m.course_unit_menu({ title }),
  addChild: () => m.course_add_unit_in_chapter(),
  addAfter: () => m.course_add_unit_after(),
  rename: () => m.course_rename_chapter(),
  move: () => m.course_move_unit(),
  deleteItem: (item) =>
    item.kind === 'chapter' ? m.course_delete_chapter() : m.course_delete_unit(),
  viewersOf: 'unit',
};

/** Wordings for `MovePageDialog`: defaults to wiki, overridden for courses. */
export interface MoveDialogLabels {
  title: (title: string) => string;
  description: () => string;
  top: () => string;
  untitled: (item: TreeItem) => string;
}

export const WIKI_MOVE_LABELS: MoveDialogLabels = {
  title: (title) => m.wiki_move_title({ title }),
  description: () => m.wiki_move_description(),
  top: () => m.wiki_move_top(),
  untitled: () => m.wiki_page_untitled(),
};

export const COURSE_MOVE_LABELS: MoveDialogLabels = {
  title: (title) => m.course_move_title({ title }),
  description: () => m.course_move_description(),
  top: () => m.course_move_top(),
  untitled: (item) =>
    item.kind === 'chapter' ? m.course_chapter_untitled() : m.course_unit_untitled(),
};

export interface RowActionContext<P extends TreeItem> {
  row: PageRow<P>;
  labels: TreeLabels;
  rules?: TreeRules<P> | undefined;
  canEdit: boolean;
  mayMove: boolean;
  keys: boolean;
  alt: string;
  unfold: (id: string) => void;
  onCreate: (draft: PageDraft) => void;
  onRename?: ((item: P) => void) | undefined;
  onMoveBy: (id: string, step: MoveStep) => void;
  onMoveTo?: ((item: P) => void) | undefined;
  onDuplicate?: ((item: P) => Promise<string | undefined>) | undefined;
  onCopied: (title: string) => void;
  onDelete?: ((item: P) => void) | undefined;
}

function moveActions<P extends TreeItem>(
  ctx: RowActionContext<P>,
  isChapter: boolean,
  hasPrior: boolean,
): MenuAction[] {
  const { row, labels } = ctx;
  const item = row.page;
  const actions: MenuAction[] = [
    {
      id: 'move-up',
      label: m.wiki_move_up(),
      icon: ArrowUpIcon,
      ...(ctx.keys ? { hint: `${ctx.alt}↑` } : {}),
      separated: hasPrior,
      disabled: row.position === 1,
      run: () => {
        ctx.onMoveBy(item.id, 'up');
      },
    },
    {
      id: 'move-down',
      label: m.wiki_move_down(),
      icon: ArrowDownIcon,
      ...(ctx.keys ? { hint: `${ctx.alt}↓` } : {}),
      disabled: row.position === row.siblings,
      run: () => {
        ctx.onMoveBy(item.id, 'down');
      },
    },
  ];
  // Chapters stay at the top level, so only non-chapter rows offer "Move…".
  if (ctx.onMoveTo && !isChapter) {
    const ask = ctx.onMoveTo;
    actions.push({
      id: 'move',
      label: labels.move(),
      icon: FolderInputIcon,
      run: () => {
        ask(item);
      },
    });
  }
  return actions;
}

/** Builds the context menu actions for one row of `PageTree`. */
export function buildTreeActions<P extends TreeItem>(ctx: RowActionContext<P>): MenuAction[] {
  const { row, labels } = ctx;
  const item = row.page;
  const isChapter = item.kind === 'chapter';
  const actions: MenuAction[] = [];

  if (isChapter || item.kind === undefined) {
    actions.push({
      id: 'add-child',
      label: labels.addChild(),
      icon: ListPlusIcon,
      run: () => {
        ctx.unfold(item.id);
        ctx.onCreate({ parentId: item.id });
      },
    });
  }
  if (!isChapter) {
    actions.push({
      id: 'add-after',
      label: labels.addAfter(),
      icon: FilePlusIcon,
      run: () => {
        ctx.onCreate({ parentId: row.parentId ?? '', after: item.id });
      },
    });
  }
  if (isChapter && ctx.onRename && labels.rename) {
    const rename = ctx.onRename;
    actions.push({
      id: 'rename',
      label: labels.rename(),
      icon: PencilIcon,
      run: () => {
        rename(item);
      },
    });
  }
  if (ctx.mayMove) actions.push(...moveActions(ctx, isChapter, actions.length > 0));
  if (ctx.canEdit && ctx.onDuplicate && !isChapter) {
    const copy = ctx.onDuplicate;
    actions.push({
      id: 'duplicate',
      label: m.wiki_duplicate_page(),
      icon: CopyIcon,
      run: () => {
        void copy(item).then((title) => {
          if (title !== undefined) ctx.onCopied(title);
        });
      },
    });
  }
  if (ctx.onDelete) {
    const remove = ctx.onDelete;
    actions.push({
      id: 'delete',
      label: labels.deleteItem(item),
      icon: Trash2Icon,
      destructive: true,
      separated: true,
      run: () => {
        remove(item);
      },
    });
  }
  return actions;
}
