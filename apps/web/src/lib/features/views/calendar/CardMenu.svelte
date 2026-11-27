<!--
  The "…" of a card in a list of the calendar (the cards of a day, the cards
  without a date). Its first line changes the card's date without a drag:
  "Change date", or "Set date" for a card that has none. It is missing where
  the calendar is placed by a date nobody can set (when cards were made or
  changed). Then duplicate and delete, as on a board; undo brings a deleted
  card back.

  Only for someone who may edit. Where the device can hover, the button waits
  for its line to be pointed at or focused; anywhere else it is always there.
  The line carries the class `group/card`.

  A list can hold hundreds of lines: each is a plain button until it is
  pointed at or pressed, and only then the menu behind it is made (in its
  place, with the focus moved onto it), so a long list does not build a menu
  for every line.
-->
<script lang="ts">
  import CalendarClockIcon from '@lucide/svelte/icons/calendar-clock';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import TrashIcon from '@lucide/svelte/icons/trash-2';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Card } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';

  interface Props {
    board: BoardStore;
    card: Card;
    title: string;
    /** The card has a date to change; else it is given one. */
    dated: boolean;
    /** Opens the picker of a day for the card; none where dates cannot be set. */
    onChangeDate?: (cardId: string) => void;
    /** The id of what the button is described by: why the date cannot be changed here. */
    describedBy?: string;
    /**
     * Out of the order of the Tab key, and shown only while its line is
     * pointed at or has the focus, on any device: where the line opens the
     * menu by a key, and the button lies over the line's own text.
     */
    quiet?: boolean;
  }

  let { board, card, title, dated, onChangeDate, describedBy, quiet = false }: Props = $props();

  const shown = $derived(
    quiet
      ? 'pointer-events-none bg-background opacity-0 group-focus-within/card:pointer-events-auto group-focus-within/card:opacity-100 aria-expanded:pointer-events-auto aria-expanded:opacity-100 can-hover:group-hover/card:pointer-events-auto can-hover:group-hover/card:opacity-100'
      : 'can-hover:opacity-0 can-hover:group-focus-within/card:opacity-100 can-hover:group-hover/card:opacity-100 can-hover:focus-visible:opacity-100 can-hover:aria-expanded:opacity-100',
  );

  let open = $state(false);
  /** The menu is made: the line was pointed at, or the button pressed. */
  let armed = $state(false);

  /** Pressed before it was pointed at (the keys, a finger): the menu is made and opened. */
  async function arm(event: MouseEvent): Promise<void> {
    const holder = (event.currentTarget as HTMLElement | null)?.parentElement;
    armed = true;
    await tick();
    holder?.querySelector<HTMLElement>('[data-card-menu]')?.focus();
    open = true;
  }

  const actions = $derived.by((): MenuAction[] => {
    const list: MenuAction[] = [];
    if (onChangeDate) {
      const change = onChangeDate;
      list.push({
        id: 'change-date',
        label: dated ? m.calendar_change_date() : m.calendar_set_date(),
        icon: CalendarClockIcon,
        afterClose: () => {
          change(card.id);
        },
      });
    }
    list.push(
      {
        id: 'duplicate',
        label: m['CardActionsMenu.duplicate'](),
        icon: CopyIcon,
        run: () => void board.duplicateCard(card.id),
      },
      {
        id: 'delete',
        label: m['CardActionsMenu.delete'](),
        icon: TrashIcon,
        destructive: true,
        separated: true,
        run: () => void board.deleteCards([card.id]),
      },
    );
    return list;
  });
</script>

{#snippet button(props: Record<string, unknown>)}
  <IconButton
    {...props}
    label={m.kanban_card_menu({ title })}
    aria-describedby={describedBy}
    tabindex={quiet ? -1 : undefined}
    data-card-menu={card.id}
    class={['shrink-0 text-muted-foreground', shown]}
  >
    <EllipsisIcon aria-hidden="true" />
  </IconButton>
{/snippet}

<span class="contents">
  {#if armed}
    <ActionMenu bind:open title={m.kanban_card_menu({ title })} subtitle={title} {actions}>
      {#snippet trigger(props: Record<string, unknown>)}
        {@render button(props)}
      {/snippet}
    </ActionMenu>
  {:else}
    {@render button({
      'aria-haspopup': 'menu',
      onpointerenter: () => {
        armed = true;
      },
      onclick: (event: MouseEvent) => void arm(event),
    })}
  {/if}
</span>
