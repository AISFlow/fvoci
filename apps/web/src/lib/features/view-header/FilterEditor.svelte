<!--
  The filter of a view: its conditions, one per line, and whether a card has
  to meet all of them or any one.

  Every change is saved as it is made (`board.setViewFields`), so each is one
  step of undo. A filter can hold groups of conditions inside it; this editor
  neither makes nor changes them, says that they are there, and keeps them as
  they are whatever else is edited (filter-edit.ts).
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { arrowChoice } from '$lib/components/arrow-choice.ts';
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
    clauseEntries,
    nestedGroupCount,
    removeClause,
    setClauseCondition,
    setClauseProperty,
    setClauseValues,
    setOperation,
    type ClauseEntry,
  } from './filter-edit.ts';
  import FilterRow from './FilterRow.svelte';

  let { board, view }: { board: BoardStore; view: BoardView } = $props();

  const templates = $derived(board.board?.cardProperties ?? []);
  const filter = $derived(view.fields.filter);
  const entries = $derived(clauseEntries(filter));
  const nested = $derived(nestedGroupCount(filter));
  const people = $derived(board.people);

  let root = $state<HTMLElement | null>(null);

  function save(next: FilterGroup): void {
    if (next === filter) return;
    void board.setViewFields(view.id, { filter: next }, 'filter');
  }

  /**
   * Applies a change to one line. A line that is no longer the condition the
   * change was made on (it was removed, or somebody else changed the filter
   * meanwhile) is left alone: a place in the list is not an identity.
   */
  function edit(
    entry: ClauseEntry,
    change: (group: FilterGroup, index: number) => FilterGroup,
  ): void {
    if (filter.filters[entry.index] !== entry.clause) return;
    save(change(filter, entry.index));
  }

  async function remove(entry: ClauseEntry): Promise<void> {
    edit(entry, removeClause);
    await tick();
    root?.querySelector<HTMLElement>('[data-tool="add-filter"]')?.focus();
  }

  const OPERATIONS: readonly { operation: FilterGroupOperation; label: () => string }[] = [
    { operation: 'and', label: m.viewheader_filter_match_all },
    { operation: 'or', label: m.viewheader_filter_match_any },
  ];
</script>

<!-- In a popover it has a width of its own, wide enough for a line in one row; a sheet gives it the screen's. -->
<div
  bind:this={root}
  class="flex max-w-full flex-col gap-3 p-2 group-data-[presentation=popover]/surface:w-[32rem]"
  data-testid="filter-editor"
>
  {#if entries.length + nested > 1}
    <div
      role="radiogroup"
      aria-label={m.viewheader_filter_match()}
      {@attach arrowChoice()}
      class="flex gap-1 self-start rounded-lg bg-muted p-0.5"
    >
      {#each OPERATIONS as { operation, label } (operation)}
        <button
          type="button"
          role="radio"
          aria-checked={filter.operation === operation}
          data-operation={operation}
          class="h-7 rounded-md px-2.5 text-sm font-medium text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring aria-checked:bg-background aria-checked:text-foreground aria-checked:shadow-sm roomy:h-touch roomy:px-3 roomy:text-base"
          onclick={() => {
            save(setOperation(filter, operation));
          }}
        >
          {label()}
        </button>
      {/each}
    </div>
  {/if}

  {#if entries.length === 0}
    <p class="px-1 text-sm text-muted-foreground">{m.viewheader_filter_none()}</p>
  {:else}
    <ul class="m-0 flex list-none flex-col gap-3 p-0" aria-label={m['ViewHeader.filter']()}>
      {#each entries as entry (entry.index)}
        <FilterRow
          clause={entry.clause}
          {templates}
          {people}
          named={board.workspace.users}
          locale={board.workspace.locale}
          regional={board.workspace.regional}
          onProperty={(propertyId: string) => {
            edit(entry, (group, index) => setClauseProperty(group, index, propertyId, templates));
          }}
          onCondition={(condition: FilterCondition) => {
            edit(entry, (group, index) => setClauseCondition(group, index, condition));
          }}
          onValues={(values: string[]) => {
            edit(entry, (group, index) => setClauseValues(group, index, values));
          }}
          onRemove={() => void remove(entry)}
        />
      {/each}
    </ul>
  {/if}

  {#if nested > 0}
    <p class="px-1 text-xs text-muted-foreground" data-testid="filter-nested">
      {m.viewheader_filter_nested({ count: nested })}
    </p>
  {/if}

  <Button
    variant="ghost"
    data-tool="add-filter"
    class="self-start roomy:h-touch roomy:text-base"
    onclick={() => {
      save(addClause(filter, templates));
    }}
  >
    {m['FilterComponent.add-filter']()}
  </Button>
</div>
