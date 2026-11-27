<!--
  The row that starts a group of the table: the value the group stands for,
  how many cards it has, a button that folds the group and one that makes a
  card with that value.

  The row is as wide as the table; what it says stays at the left edge while
  the table is scrolled sideways.
-->
<script lang="ts">
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import PlusIcon from '@lucide/svelte/icons/plus';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import OptionChip from '$lib/components/option-chip/OptionChip.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { PropertyColor } from '$lib/logic/wire.ts';

  import { CELL_FOCUS, ROW_HEIGHT } from './host.ts';

  interface Props {
    /** Names the row for the arrow keys and the tests. */
    rowId: string;
    /** What the group is called: the option, the person, or "No Status". */
    name: string;
    /** The colour name of the option; undefined draws the name without a chip. */
    color: PropertyColor | undefined;
    count: number;
    collapsed: boolean;
    /** The reader may add a card with the group's value. */
    canAdd: boolean;
    /** A dragged row would land in this group, which shows no rows to land between. */
    dropInto: boolean;
    onToggle: () => void;
    onAdd: () => void;
  }

  let { rowId, name, color, count, collapsed, canAdd, dropInto, onToggle, onAdd }: Props = $props();
</script>

<div
  role="row"
  data-nav-row
  data-row={rowId}
  data-group-header
  data-drop-into={dropInto ? '' : undefined}
  class={['flex border-t bg-muted/40', dropInto && 'ring-2 ring-ring ring-inset']}
>
  <div
    role="gridcell"
    tabindex="-1"
    data-cell
    data-kind="group"
    data-col="*"
    class={['sticky start-0 flex items-center gap-1.5 ps-1 pe-2', ROW_HEIGHT, CELL_FOCUS]}
  >
    <IconButton
      label={collapsed ? m.table_group_expand({ name }) : m.table_group_collapse({ name })}
      aria-expanded={!collapsed}
      data-activate
      data-inner
      tabindex={-1}
      class="size-7"
      onclick={onToggle}
    >
      {#if collapsed}
        <ChevronRightIcon class="rtl:rotate-180" aria-hidden="true" />
      {:else}
        <ChevronDownIcon aria-hidden="true" />
      {/if}
    </IconButton>
    {#if color === undefined}
      <span class="max-w-64 min-w-0 truncate font-medium" data-slot="group-name">{name}</span>
    {:else}
      <span class="max-w-64 min-w-0" data-slot="group-name"><OptionChip {name} {color} /></span>
    {/if}
    <span class="text-xs text-muted-foreground tabular-nums" data-slot="group-count">
      <span aria-hidden="true">{count}</span>
      <span class="sr-only">{m.table_group_count({ count })}</span>
    </span>
    {#if canAdd}
      <IconButton
        label={m.table_group_add({ name })}
        data-inner
        data-group-add
        tabindex={-1}
        class="size-7"
        onclick={onAdd}
      >
        <PlusIcon aria-hidden="true" />
      </IconButton>
    {/if}
  </div>
</div>
