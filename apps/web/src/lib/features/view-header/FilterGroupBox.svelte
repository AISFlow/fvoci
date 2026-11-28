<!--
  A group of conditions in the filter: a box of its own among the lines of
  the filter, with whether a card has to meet all of its conditions or one
  of them, its conditions, and a button that adds one to it. Its menu takes
  it apart (its conditions take its place in the filter) or deletes it with
  its conditions.

  A group inside it is deeper than the editor goes: it is said to be there
  and kept as it is whatever else is edited (filter-edit.ts).

  The lines of its conditions are the editor's (`row`): they change the
  filter as the lines outside a group do. Groups are numbered in their order
  (`number`), so that each is told from the others by its name alone.
-->
<script lang="ts">
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import type { Snippet } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { FilterGroupOperation } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { pathKey, type ClauseEntry, type GroupEntry } from './filter-edit.ts';
  import FilterOperation from './FilterOperation.svelte';

  interface Props {
    entry: GroupEntry;
    /** Which group of the filter it is, from 1. */
    number: number;
    /** Draws the line of one of its conditions. */
    row: Snippet<[ClauseEntry]>;
    onOperation: (operation: FilterGroupOperation) => void;
    onAdd: () => void;
    onUngroup: () => void;
    onRemove: () => void;
  }

  let { entry, number, row, onOperation, onAdd, onUngroup, onRemove }: Props = $props();

  const clauses = $derived(entry.entries.filter((child) => child.kind === 'clause'));
  const kept = $derived(entry.entries.filter((child) => child.kind === 'kept').length);

  const actions = $derived<MenuAction[]>([
    { id: 'ungroup', label: m.viewheader_filter_ungroup(), run: onUngroup },
    {
      id: 'remove-group',
      label: m.viewheader_filter_remove_group(),
      destructive: true,
      run: onRemove,
    },
  ]);
</script>

<li
  class="flex min-w-0 flex-col gap-2 rounded-lg border border-border p-2"
  data-filter-group={pathKey(entry.path)}
>
  <div class="flex min-w-0 items-center justify-between gap-2">
    {#if entry.group.filters.length > 1}
      <FilterOperation
        operation={entry.group.operation}
        label={m.viewheader_filter_group_match({ number })}
        onChange={onOperation}
      />
    {:else}
      <span class="min-w-0 truncate px-1 text-sm text-muted-foreground">
        {m.viewheader_filter_group({ number })}
      </span>
    {/if}
    <ActionMenu title={m.viewheader_filter_group_menu({ number })} {actions} align="end">
      {#snippet trigger(props: Record<string, unknown>)}
        <IconButton
          {...props}
          label={m.viewheader_filter_group_menu({ number })}
          data-slot="group-menu"
        >
          <EllipsisIcon />
        </IconButton>
      {/snippet}
    </ActionMenu>
  </div>

  <ul
    class="m-0 flex list-none flex-col gap-3 p-0"
    aria-label={m.viewheader_filter_group({ number })}
  >
    {#each clauses as clause (pathKey(clause.path))}
      {@render row(clause)}
    {/each}
  </ul>

  {#if kept > 0}
    <p class="px-1 text-xs text-muted-foreground" data-testid="filter-nested">
      {m.viewheader_filter_nested({ count: kept })}
    </p>
  {/if}

  <Button
    variant="ghost"
    size="sm"
    data-slot="add-to-group"
    class="self-start roomy:h-touch roomy:text-base"
    onclick={onAdd}
  >
    <PlusIcon data-icon="inline-start" />
    {m.viewheader_filter_group_add({ number })}
  </Button>
</li>
