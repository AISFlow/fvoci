<!--
  Parent card breadcrumb and parent picker at the top of an ordinary card's detail.

  When the card is a sub-card (depth 2 or 3), displays its ancestor trail
  (grandparent -> parent) so the reader can jump directly to any parent card.
  When the reader may edit the board, offers a searchable picker (`eligibleParentsFor`)
  to set or change the parent card, plus a one-click control to clear it back
  to the top level.
-->
<script lang="ts">
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import CornerLeftUpIcon from '@lucide/svelte/icons/corner-left-up';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import XIcon from '@lucide/svelte/icons/x';

  import CardNumber from '$lib/components/card-number/CardNumber.svelte';
  import PickList from '$lib/components/pick-list/PickList.svelte';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { overlayPresentation } from '$lib/layout/presentation.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import { ancestorsOf, eligibleParentsFor } from '$lib/logic/card-hierarchy.ts';
  import type { BoardStore, CardSummary } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import {
    filterPickerCandidates,
    HIERARCHY_PICKER_LIMIT,
    pickerSearchTexts,
    sortPickerCandidates,
  } from './hierarchy-picker.ts';

  interface Props {
    board: BoardStore;
    card: CardSummary;
    onOpenCard?: (cardId: string) => void;
  }

  let { board, card, onOpenCard }: Props = $props();

  let open = $state(false);
  let search = $state('');

  const editable = $derived(board.canEdit);
  const presentation = $derived(overlayPresentation(page.stage));
  const untitled = $derived(m['KanbanCard.untitled']());
  const ancestors = $derived(ancestorsOf(card.id, board.cardsById));
  const hasParent = $derived(ancestors.length > 0);
  const selectedIds = $derived(card.parentId === '' ? [] : [card.parentId]);

  const sortedCandidates = $derived.by(() =>
    open
      ? sortPickerCandidates(
          eligibleParentsFor(card.id, board.realCards),
          board.workspace.locale,
          untitled,
        )
      : [],
  );
  const matches = $derived(filterPickerCandidates(sortedCandidates, search));
  const listed = $derived(matches.slice(0, HIERARCHY_PICKER_LIMIT));

  async function setParent(parentId: string): Promise<void> {
    open = false;
    search = '';
    if (card.parentId === parentId) return;
    await board.changeCard(card.id, (current) => ({ ...current, parentId }), 'changeParent');
  }
</script>

{#if hasParent || editable}
  <div
    class="flex flex-wrap items-center gap-1.5 text-sm text-muted-foreground"
    data-testid="card-parent-bar"
  >
    {#if hasParent}
      <nav
        aria-label={m.card_parent_breadcrumb()}
        data-testid="card-parent-breadcrumb"
        class="flex min-w-0 flex-wrap items-center gap-1"
      >
        <CornerLeftUpIcon class="size-3.5 shrink-0" aria-hidden="true" />
        {#each ancestors as ancestor, index (ancestor.id)}
          {#if index > 0}
            <ChevronRightIcon class="size-3.5 shrink-0" aria-hidden="true" />
          {/if}
          {#if onOpenCard}
            <button
              type="button"
              data-parent-crumb={ancestor.id}
              class="inline-flex max-w-56 min-w-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-touch"
              onclick={() => {
                onOpenCard(ancestor.id);
              }}
            >
              <CardNumber number={ancestor.number} />
              {#if ancestor.fields.icon}
                <span class="shrink-0" aria-hidden="true">{ancestor.fields.icon}</span>
              {/if}
              <span class="truncate">{ancestor.title || untitled}</span>
            </button>
          {:else}
            <span
              data-parent-crumb={ancestor.id}
              class="inline-flex max-w-56 min-w-0 items-center gap-1 px-1.5 py-0.5 text-foreground"
            >
              <CardNumber number={ancestor.number} />
              {#if ancestor.fields.icon}
                <span class="shrink-0" aria-hidden="true">{ancestor.fields.icon}</span>
              {/if}
              <span class="truncate">{ancestor.title || untitled}</span>
            </span>
          {/if}
        {/each}
      </nav>
    {/if}

    {#if editable}
      <PickerSurface
        bind:open
        {presentation}
        title={hasParent ? m.card_parent_change() : m.card_parent_set()}
        onOpenChange={(next) => {
          if (!next) search = '';
        }}
      >
        {#snippet trigger({ props }: { props: Record<string, unknown> })}
          {#if hasParent}
            <button
              {...props}
              type="button"
              data-testid="card-parent-picker-trigger"
              aria-label={m.card_parent_change()}
              title={m.card_parent_change()}
              class="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground aria-expanded:bg-muted roomy:size-touch"
            >
              <PencilIcon class="size-3.5" aria-hidden="true" />
            </button>
          {:else}
            <button
              {...props}
              type="button"
              data-testid="card-parent-picker-trigger"
              class="inline-flex min-h-7 items-center gap-1.5 rounded-md px-2 py-0.5 text-xs text-muted-foreground hover:bg-muted hover:text-foreground aria-expanded:bg-muted roomy:min-h-touch"
            >
              <CornerLeftUpIcon class="size-3.5 shrink-0" aria-hidden="true" />
              <span>{m.card_parent_set()}</span>
            </button>
          {/if}
        {/snippet}

        <div class="flex flex-col gap-1">
          <PickList
            bind:search
            items={listed}
            textOf={(item: CardSummary) => pickerSearchTexts(item, untitled)}
            selected={selectedIds}
            label={hasParent ? m.card_parent_change() : m.card_parent_set()}
            placeholder={m.card_parent_search()}
            emptyText={m.card_parent_empty()}
            onPick={(item: CardSummary) => void setParent(item.id)}
          >
            {#snippet row(item: CardSummary)}
              <CardNumber number={item.number} />
              {#if item.fields.icon}
                <span class="shrink-0" aria-hidden="true">{item.fields.icon}</span>
              {/if}
              <span class="min-w-0 truncate">{item.title || untitled}</span>
            {/snippet}
          </PickList>
          {#if matches.length > listed.length}
            <p class="px-2 py-1 text-xs text-muted-foreground">
              {m.card_hierarchy_more({ count: matches.length - listed.length })}
            </p>
          {/if}
          {#if hasParent}
            <Button
              variant="ghost"
              class="justify-start group-data-[presentation=sheet]/surface:min-h-touch"
              data-testid="card-parent-picker-clear"
              onclick={() => void setParent('')}
            >
              {m.card_parent_clear()}
            </Button>
          {/if}
        </div>
      </PickerSurface>

      {#if hasParent}
        <button
          type="button"
          data-testid="card-parent-clear"
          aria-label={m.card_parent_clear()}
          title={m.card_parent_clear()}
          class="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground roomy:size-touch"
          onclick={() => void setParent('')}
        >
          <XIcon class="size-3.5" aria-hidden="true" />
        </button>
      {/if}
    {/if}
  </div>
{/if}
