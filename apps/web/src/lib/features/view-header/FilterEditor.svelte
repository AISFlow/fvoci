<!--
  The filter of a view: its conditions, one per line, its groups of
  conditions, each a box of its own (FilterGroupBox.svelte), and whether a
  card has to meet all of them or any one.

  Every change is saved as it is made (`board.setViewFields`), so each is one
  step of undo. The editor makes groups one level deep: a group in the
  filter, with conditions in it. A filter written elsewhere can hold groups
  deeper than that; the editor says that they are there and keeps them as
  they are whatever else is edited (filter-edit.ts).
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type {
    BoardView,
    FilterCondition,
    FilterGroup,
    FilterGroupOperation,
  } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import {
    addClause,
    addGroup,
    filterEntries,
    nodeAt,
    pathKey,
    removeClause,
    removeGroup,
    setClauseCondition,
    setClauseProperty,
    setClauseValues,
    setOperation,
    ungroup,
    type ClauseEntry,
    type FilterPath,
    type GroupEntry,
  } from './filter-edit.ts';
  import FilterGroupBox from './FilterGroupBox.svelte';
  import FilterOperation from './FilterOperation.svelte';
  import FilterRow from './FilterRow.svelte';

  let { board, view }: { board: BoardStore; view: BoardView } = $props();

  const templates = $derived(board.board?.cardProperties ?? []);
  const doneWhen = $derived(board.board?.doneWhen);
  const filter = $derived(view.fields.filter);
  const entries = $derived(filterEntries(filter));
  /** The groups of the filter in their order, which numbers them. */
  const groups = $derived(entries.filter((entry) => entry.kind === 'group'));
  const people = $derived(board.people);

  let root = $state<HTMLElement | null>(null);

  function save(next: FilterGroup): void {
    if (next === filter) return;
    void board.setViewFields(view.id, { filter: next }, 'filter');
  }

  /**
   * Applies a change to one part of the filter. A part that is no longer the
   * one the change was made on (it was removed, or somebody else changed the
   * filter meanwhile) is left alone: a place in the list is not an identity.
   */
  function edit(
    entry: ClauseEntry | GroupEntry,
    change: (group: FilterGroup, path: FilterPath) => FilterGroup,
  ): void {
    const part = entry.kind === 'clause' ? entry.clause : entry.group;
    if (nodeAt(filter, entry.path) !== part) return;
    save(change(filter, entry.path));
  }

  /** After a line or a group is gone, the focus goes to "Add a condition". */
  async function focusAdd(): Promise<void> {
    await tick();
    root?.querySelector<HTMLElement>('[data-tool="add-filter"]')?.focus();
  }
</script>

{#snippet row(entry: ClauseEntry)}
  <FilterRow
    clause={entry.clause}
    {templates}
    {doneWhen}
    {people}
    named={board.workspace.users}
    locale={board.workspace.locale}
    regional={board.workspace.regional}
    onProperty={(propertyId: string) => {
      edit(entry, (group, path) => setClauseProperty(group, path, propertyId, templates));
    }}
    onCondition={(condition: FilterCondition) => {
      edit(entry, (group, path) => setClauseCondition(group, path, condition));
    }}
    onValues={(values: string[]) => {
      edit(entry, (group, path) => setClauseValues(group, path, values));
    }}
    onRemove={() => {
      edit(entry, removeClause);
      void focusAdd();
    }}
  />
{/snippet}

<!-- In a popover it has a width of its own, wide enough for a line in one row; a sheet gives it the screen's. -->
<div
  bind:this={root}
  class="flex max-w-full min-w-0 flex-col gap-3 p-2 group-data-[presentation=popover]/surface:w-[32rem]"
  data-testid="filter-editor"
>
  {#if filter.filters.length > 1}
    <FilterOperation
      operation={filter.operation}
      label={m.viewheader_filter_match()}
      onChange={(operation: FilterGroupOperation) => {
        save(setOperation(filter, [], operation));
      }}
    />
  {/if}

  {#if entries.length === 0}
    <p class="px-1 text-sm text-muted-foreground">{m.viewheader_filter_none()}</p>
  {:else}
    <ul class="m-0 flex list-none flex-col gap-3 p-0" aria-label={m['ViewHeader.filter']()}>
      {#each entries as entry (pathKey(entry.path))}
        {#if entry.kind === 'clause'}
          {@render row(entry)}
        {:else if entry.kind === 'group'}
          <FilterGroupBox
            {entry}
            number={groups.indexOf(entry) + 1}
            {row}
            onOperation={(operation: FilterGroupOperation) => {
              edit(entry, (group, path) => setOperation(group, path, operation));
            }}
            onAdd={() => {
              edit(entry, (group, path) => addClause(group, templates, path));
            }}
            onUngroup={() => {
              edit(entry, ungroup);
              void focusAdd();
            }}
            onRemove={() => {
              edit(entry, removeGroup);
              void focusAdd();
            }}
          />
        {/if}
      {/each}
    </ul>
  {/if}

  <div class="flex flex-wrap gap-1">
    <Button
      variant="ghost"
      data-tool="add-filter"
      class="roomy:h-touch roomy:text-base"
      onclick={() => {
        save(addClause(filter, templates));
      }}
    >
      {m['FilterComponent.add-filter']()}
    </Button>
    <Button
      variant="ghost"
      data-tool="add-group"
      class="roomy:h-touch roomy:text-base"
      onclick={() => {
        save(addGroup(filter, templates));
      }}
    >
      {m.viewheader_filter_add_group()}
    </Button>
  </div>
</div>
