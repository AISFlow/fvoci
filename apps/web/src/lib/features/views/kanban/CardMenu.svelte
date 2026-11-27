<!--
  The "…" of a card on a board or in a gallery: duplicate, copy its link,
  and for someone who may edit, select it, move it to another column and
  delete it. Deleting does not ask first: undo brings the card back.

  "Move to another column" is the way between columns that needs no
  drag. Choosing it opens a second menu with the columns, at the same
  place.

  Where the device can hover, the button waits for the card to be pointed at
  or focused; anywhere else it is always there. The element around the card
  carries the class `group/card`.
-->
<script lang="ts">
  import ArrowRightLeftIcon from '@lucide/svelte/icons/arrow-right-left';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import LinkIcon from '@lucide/svelte/icons/link';
  import SquareCheckIcon from '@lucide/svelte/icons/square-check';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import { onDestroy } from 'svelte';

  import { copyCardLink } from '$lib/components/card-number/card-link.ts';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Card } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';

  import { CARD_MENU_BUTTON, type MoveTarget } from './kanban.ts';

  interface Props {
    open?: boolean;
    board: BoardStore;
    card: Card;
    /** The address of the card, for "Copy link". */
    href: string;
    /**
     * The cards "delete" and "move" act on: the card alone, or the selection
     * it is part of.
     */
    targetIds?: readonly string[];
    /** The columns of the board. Without them there is no "Move to another column". */
    columns?: readonly MoveTarget[];
    onMove?: (optionId: string) => void;
    /** Whether the card is selected. Without `onSelect` there is no "Select". */
    selected?: boolean;
    onSelect?: () => void;
  }

  let {
    open = $bindable(false),
    board,
    card,
    href,
    targetIds,
    columns = [],
    onMove,
    selected = false,
    onSelect,
  }: Props = $props();

  let moveMenuMade = $state(false);
  let moving = $state(false);
  let copied = $state(false);
  let holder = $state<HTMLElement | null>(null);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  const title = $derived(card.title || m['KanbanCard.untitled']());
  const label = $derived(m.kanban_card_menu({ title }));
  const targets = $derived(targetIds ?? [card.id]);
  const many = $derived(targets.length > 1 ? String(targets.length) : undefined);

  async function copyLink(): Promise<void> {
    try {
      await copyCardLink(new URL(href, location.href).href, card);
    } catch {
      // The browser refused (no permission, not a secure page): nothing was copied, and nothing says it was.
      return;
    }
    copied = true;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
      copied = false;
    }, 2000);
  }

  onDestroy(() => {
    clearTimeout(copiedTimer);
  });

  const actions = $derived.by((): MenuAction[] => {
    const list: MenuAction[] = [];
    if (board.canEdit) {
      list.push({
        id: 'duplicate',
        label: m['CardActionsMenu.duplicate'](),
        icon: CopyIcon,
        run: () => void board.duplicateCard(card.id),
      });
    }
    list.push({
      id: 'copy-link',
      label: m['CardActionsMenu.copyLink'](),
      icon: LinkIcon,
      run: () => void copyLink(),
    });
    if (!board.canEdit) return list;
    if (onSelect) {
      list.push({
        id: 'select',
        label: selected ? m.kanban_deselect() : m.kanban_select(),
        icon: SquareCheckIcon,
        run: onSelect,
      });
    }
    if (onMove && columns.length > 1) {
      list.push({
        id: 'move',
        label: m.kanban_move_to_column(),
        icon: ArrowRightLeftIcon,
        hint: many,
        run: () => {
          moveMenuMade = true;
        },
        afterClose: () => {
          moving = true;
        },
      });
    }
    list.push({
      id: 'delete',
      label: m['CardActionsMenu.delete'](),
      icon: TrashIcon,
      hint: many,
      destructive: true,
      separated: true,
      run: () => void board.deleteCards(targets),
    });
    return list;
  });

  const moveActions = $derived(
    columns.map((column): MenuAction => ({
      id: `to-${column.optionId}`,
      label: column.name,
      checked: column.current,
      // A card alone has nowhere to go in the column it is in.
      disabled: column.current && targets.length === 1,
      run: () => onMove?.(column.optionId),
    })),
  );
</script>

<span bind:this={holder} class="relative block">
  <ActionMenu bind:open title={label} {actions}>
    {#snippet trigger(props: Record<string, unknown>)}
      <button
        {...props}
        type="button"
        aria-label={label}
        title={label}
        data-card-menu={card.id}
        class={CARD_MENU_BUTTON}
      >
        <svg class="size-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
          <circle cx="3" cy="8" r="1.25" /><circle cx="8" cy="8" r="1.25" />
          <circle cx="13" cy="8" r="1.25" />
        </svg>
      </button>
    {/snippet}
  </ActionMenu>
  {#if moveMenuMade && moveActions.length > 1}
    <ActionMenu
      bind:open={moving}
      title={m.kanban_move_to_column()}
      subtitle={title}
      actions={moveActions}
      returnFocus={() => holder?.querySelector('button')}
    >
      {#snippet trigger(props: Record<string, unknown>)}
        <span
          {...props}
          aria-hidden="true"
          data-move-anchor
          class="pointer-events-none absolute inset-0"
        ></span>
      {/snippet}
    </ActionMenu>
  {/if}
  <span class="sr-only" role="status" data-testid="link-copied">
    {copied ? m['CardActionsMenu.copiedLink']() : ''}
  </span>
  {#if copied}
    <span
      aria-hidden="true"
      class="absolute top-full right-0 z-10 mt-1 rounded-md bg-foreground px-2 py-0.5 text-xs whitespace-nowrap text-background"
    >
      {m['CardActionsMenu.copiedLink']()}
    </span>
  {/if}
</span>
