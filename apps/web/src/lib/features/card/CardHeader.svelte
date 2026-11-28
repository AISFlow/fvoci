<!--
  The top row of an open card: how to leave it, where it belongs (the board
  and the view it was opened from, and its number there), how the saving of
  its body stands, whether the reader watches it (`WatchToggle`), and its
  menu. Deleting from the menu is not asked: undo, and the trash, take it
  back.

  Leaving is a back arrow at the left when the card fills the screen, as a
  page would have it, and a close button at the right when it is a panel or a
  dialog. Both do the same.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import LinkIcon from '@lucide/svelte/icons/link';
  import Redo2Icon from '@lucide/svelte/icons/redo-2';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import Undo2Icon from '@lucide/svelte/icons/undo-2';
  import XIcon from '@lucide/svelte/icons/x';

  import { copyCardLink } from '$lib/components/card-number/card-link.ts';
  import CardNumber from '$lib/components/card-number/CardNumber.svelte';
  import FavoriteToggle from '$lib/components/favorite-toggle/FavoriteToggle.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { markdownActions } from '$lib/components/markdown/markdown-actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CardFrame } from '$lib/layout/presentation.ts';
  import type { BoardView } from '$lib/logic/wire.ts';
  import { workspaceFavorites, type BoardStore, type CardStore } from '$lib/state/index.ts';
  import { markdownReader } from '$lib/state/markdown-types.ts';

  import WatchToggle from './WatchToggle.svelte';

  import type { SaveState } from './save-state.ts';

  interface Props {
    board: BoardStore;
    view: BoardView | undefined;
    card: CardStore;
    frame: CardFrame;
    /** How the saving of the body stands. */
    saveState: SaveState;
    /** The address of a card of the board: its link to copy. */
    cardHref: (cardId: string) => string;
    onClose: () => void;
    onDuplicate: () => void;
    /** Opens the versions kept of the body. */
    onVersions: () => void;
    onDelete: () => void;
    /** Extra actions in the card header (e.g. personal timer). */
    extraActions?: Snippet<[CardStore]> | undefined;
  }

  let {
    board,
    view,
    card,
    frame,
    saveState,
    cardHref,
    onClose,
    onDuplicate,
    onVersions,
    onDelete,
    extraActions,
  }: Props = $props();

  let menuOpen = $state(false);
  /** Said once in place of the saving state: what was copied. */
  let copied = $state<'link' | 'markdown' | undefined>();

  function say(what: 'link' | 'markdown'): void {
    copied = what;
    setTimeout(() => {
      if (copied === what) copied = undefined;
    }, 2000);
  }

  const workspace = $derived(board.workspace);
  const cardTitle = $derived(card.summary?.title || m['KanbanCard.untitled']());
  const there = $derived(card.summary !== undefined && !card.missing);
  const number = $derived(card.summary?.fields.isTemplate ? undefined : card.summary?.number);

  const SAVE_TEXT: Record<SaveState, () => string> = {
    idle: () => '',
    saving: m.card_saving,
    saved: m.card_saved,
    unsaved: m.card_not_saved,
  };

  async function copyLink(): Promise<void> {
    try {
      // A card is an address of its own: that is the link to it.
      const address = new URL(cardHref(card.cardId), location.href).href;
      await copyCardLink(address, { title: card.summary?.title ?? '', number });
    } catch {
      // No clipboard here (a page that is not served securely): nothing was copied, so nothing is said.
      return;
    }
    say('link');
  }

  const actions = $derived.by<MenuAction[]>(() => {
    const list: MenuAction[] = [];
    if (board.canComment) {
      list.push(
        {
          id: 'undo',
          label: m['UndoRedoHotKeys.canUndo'](),
          icon: Undo2Icon,
          disabled: !workspace.canUndo,
          run: () => void workspace.undoLast(),
        },
        {
          id: 'redo',
          label: m['UndoRedoHotKeys.canRedo'](),
          icon: Redo2Icon,
          disabled: !workspace.canRedo,
          run: () => void workspace.redoLast(),
        },
      );
    }
    if (there && board.canEdit) {
      list.push({
        id: 'duplicate',
        label: m['CardActionsMenu.duplicate'](),
        icon: CopyIcon,
        separated: list.length > 0,
        run: onDuplicate,
      });
    }
    if (there) {
      list.push({
        id: 'copy-link',
        label: m['CardActionsMenu.copyLink'](),
        icon: LinkIcon,
        separated: list.length > 0 && !board.canEdit,
        run: () => void copyLink(),
      });
      list.push(
        ...markdownActions({
          read: markdownReader(workspace.backend.markdown, {
            kind: 'card',
            boardId: board.boardId,
            cardId: card.cardId,
          }),
          title: card.summary?.title ?? '',
          onCopied: () => {
            say('markdown');
          },
          onRefused: (error) => {
            workspace.report('load', error);
          },
        }),
      );
    }
    // The versions of the body are for the board's members: not for a reader by its public link.
    if (there && board.role !== '') {
      list.push({
        id: 'versions',
        label: m.card_versions(),
        icon: HistoryIcon,
        run: onVersions,
      });
    }
    if (there && board.canEdit) {
      list.push({
        id: 'delete',
        label: m['CardActionsMenu.delete'](),
        icon: Trash2Icon,
        destructive: true,
        separated: true,
        run: onDelete,
      });
    }
    return list;
  });
</script>

<header
  class="flex h-12 shrink-0 items-center gap-1 px-1.5 @card-roomy/card:px-3"
  data-testid="card-header"
>
  {#if frame === 'full'}
    <IconButton label={m.ui_back()} data-testid="card-back" onclick={onClose}>
      <ArrowLeftIcon aria-hidden="true" />
    </IconButton>
  {/if}
  <p
    class="flex min-w-0 flex-1 items-center gap-1.5 px-1.5 text-muted-foreground"
    data-testid="card-location"
  >
    {#if board.board?.icon}
      <span class="shrink-0" aria-hidden="true">{board.board.icon}</span>
    {/if}
    <span class="min-w-0 shrink truncate">
      {board.board?.title || m['Sidebar.untitled-board']()}
    </span>
    {#if view}
      <ChevronRightIcon class="size-4 shrink-0" aria-hidden="true" />
      <span class="min-w-0 shrink-[2] truncate">{view.title || m['Sidebar.untitled-view']()}</span>
    {/if}
    {#if number !== undefined}
      <ChevronRightIcon class="size-4 shrink-0" aria-hidden="true" />
      <CardNumber {number} />
    {/if}
  </p>
  {#if extraActions}
    {@render extraActions(card)}
  {/if}
  {#if there && board.role !== '' && !card.summary?.fields.isTemplate}
    <WatchToggle {workspace} blockId={card.cardId} kind="card" />
  {/if}
  {#if there && !card.summary?.fields.isTemplate && !board.container?.isTemplate}
    <FavoriteToggle stars={workspaceFavorites(workspace)} targetId={card.cardId} />
  {/if}
  <span
    class="shrink-0 px-1 text-xs text-muted-foreground"
    role="status"
    aria-live="polite"
    data-testid="card-save-state"
    data-state={saveState}
  >
    {copied === 'link'
      ? m['CardActionsMenu.copiedLink']()
      : copied === 'markdown'
        ? m.markdown_copied()
        : SAVE_TEXT[saveState]()}
  </span>
  {#if actions.length > 0}
    <ActionMenu bind:open={menuOpen} title={m.card_menu()} subtitle={cardTitle} {actions}>
      {#snippet trigger(props: Record<string, unknown>)}
        <!-- The menu holds undo and redo: it says so while one of them waits for a write. -->
        <IconButton
          {...props}
          label={m.card_menu()}
          data-testid="card-menu"
          aria-busy={workspace.undoBusy}
          class="aria-busy:motion-safe:animate-pulse aria-busy:motion-reduce:opacity-60"
        >
          <EllipsisIcon aria-hidden="true" />
        </IconButton>
      {/snippet}
    </ActionMenu>
  {/if}
  {#if frame !== 'full'}
    <IconButton label={m.ui_close()} data-testid="card-close" onclick={onClose}>
      <XIcon aria-hidden="true" />
    </IconButton>
  {/if}
</header>
