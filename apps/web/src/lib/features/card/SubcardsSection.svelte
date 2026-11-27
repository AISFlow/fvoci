<!--
  Sub-cards section inside an ordinary card's detail dialog.

  Lists direct sub-cards with their number, title (click to navigate), completion /
  status indicator, nested sub-card count (for depth-2 sub-cards), and a detach action.
  Displays completion progress (done/total and progress bar) when sub-cards exist.
  When the reader can edit and the card sits below MAX_CARD_DEPTH (depth 1 or 2),
  provides two ways to add sub-cards:
    1. Inline title entry (Enter or Add) to create a brand-new sub-card.
    2. Searchable picker (`eligibleSubcardsFor`) to attach an existing card as a sub-card.
-->
<script lang="ts">
  import CheckIcon from '@lucide/svelte/icons/check';
  import Link2Icon from '@lucide/svelte/icons/link-2';
  import ListTreeIcon from '@lucide/svelte/icons/list-tree';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import UnlinkIcon from '@lucide/svelte/icons/unlink';

  import CardNumber from '$lib/components/card-number/CardNumber.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import OptionChip from '$lib/components/option-chip/OptionChip.svelte';
  import PickList from '$lib/components/pick-list/PickList.svelte';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isComposing } from '$lib/keys.ts';
  import { overlayPresentation } from '$lib/layout/presentation.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import {
    canAddSubcard,
    directChildrenOf,
    eligibleSubcardsFor,
    isCardCompleted,
    statusOptionOf,
    subcardProgress,
  } from '$lib/logic/card-hierarchy.ts';
  import type { BoardStore, CardSummary } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';

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

  const uid = $props.id();
  let newTitle = $state('');
  let creating = $state(false);
  let linkOpen = $state(false);
  let linkSearch = $state('');

  const editable = $derived(board.canEdit);
  const presentation = $derived(overlayPresentation(page.stage));
  const untitled = $derived(m['KanbanCard.untitled']());
  const properties = $derived(board.board?.cardProperties ?? []);
  const statusProp = $derived(board.statusProperty);

  const hierarchy = $derived(board.hierarchy);
  const children = $derived(directChildrenOf(card.id, hierarchy.childrenMap));
  const progress = $derived(
    subcardProgress(card.id, hierarchy.childrenMap, properties, statusProp),
  );
  const canAdd = $derived(editable && canAddSubcard(card.id, hierarchy.byId));
  const visible = $derived(children.length > 0 || canAdd);

  const sortedLinkCandidates = $derived.by(() =>
    linkOpen
      ? sortPickerCandidates(
          eligibleSubcardsFor(card.id, board.realCards, hierarchy),
          board.workspace.locale,
          untitled,
        )
      : [],
  );
  const linkMatches = $derived(filterPickerCandidates(sortedLinkCandidates, linkSearch));
  const linkListed = $derived(linkMatches.slice(0, HIERARCHY_PICKER_LIMIT));

  async function createSubcard(): Promise<void> {
    const title = newTitle.trim();
    if (title === '' || creating || !canAdd) return;
    creating = true;
    try {
      const created = await board.createCard({ title, parentId: card.id });
      if (created) newTitle = '';
    } finally {
      creating = false;
    }
  }

  async function attachExisting(childId: string): Promise<void> {
    linkOpen = false;
    linkSearch = '';
    await board.changeCard(
      childId,
      (current) => ({ ...current, parentId: card.id }),
      'changeParent',
    );
  }

  async function detachSubcard(childId: string): Promise<void> {
    await board.changeCard(childId, (current) => ({ ...current, parentId: '' }), 'changeParent');
  }

  function onNewKeyDown(event: KeyboardEvent): void {
    if (isComposing(event)) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      void createSubcard();
    } else if (event.key === 'Escape' && newTitle !== '') {
      event.preventDefault();
      event.stopPropagation();
      newTitle = '';
    }
  }
</script>

{#if visible}
  <section
    aria-labelledby="{uid}-heading"
    class="flex flex-col gap-3 border-t pt-5"
    data-testid="card-subcards"
  >
    <div class="flex min-h-8 flex-wrap items-center gap-2">
      <h3 id="{uid}-heading" class="flex min-w-0 flex-1 items-center gap-2 font-semibold">
        <ListTreeIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span>{m.card_subcards()}</span>
        {#if progress.total > 0}
          <span class="text-sm font-normal text-muted-foreground" data-testid="subcards-progress">
            {progress.done}/{progress.total} ({progress.percent}%)
          </span>
        {/if}
      </h3>

      {#if canAdd}
        <PickerSurface
          bind:open={linkOpen}
          {presentation}
          title={m.card_subcard_link()}
          onOpenChange={(next) => {
            if (!next) linkSearch = '';
          }}
        >
          {#snippet trigger({ props }: { props: Record<string, unknown> })}
            <Button
              {...props}
              variant="outline"
              size="sm"
              class="roomy:min-h-touch"
              data-testid="subcard-link-trigger"
            >
              <Link2Icon data-icon="inline-start" aria-hidden="true" />
              {m.card_subcard_link()}
            </Button>
          {/snippet}

          <div class="flex flex-col gap-1">
            <PickList
              bind:search={linkSearch}
              items={linkListed}
              textOf={(item: CardSummary) => pickerSearchTexts(item, untitled)}
              selected={[]}
              label={m.card_subcard_link()}
              placeholder={m.card_subcard_link_search()}
              emptyText={m.card_subcard_link_empty()}
              onPick={(item: CardSummary) => void attachExisting(item.id)}
            >
              {#snippet row(item: CardSummary)}
                <CardNumber number={item.number} />
                {#if item.fields.icon}
                  <span class="shrink-0" aria-hidden="true">{item.fields.icon}</span>
                {/if}
                <span class="min-w-0 truncate">{item.title || untitled}</span>
              {/snippet}
            </PickList>
            {#if linkMatches.length > linkListed.length}
              <p class="px-2 py-1 text-xs text-muted-foreground">
                {m.card_hierarchy_more({ count: linkMatches.length - linkListed.length })}
              </p>
            {/if}
          </div>
        </PickerSurface>
      {/if}
    </div>

    {#if progress.total > 0}
      <div
        role="progressbar"
        aria-label={m.card_subcards_progress({ done: progress.done, total: progress.total })}
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={progress.percent}
        data-testid="subcards-progress-bar"
        class="h-1.5 overflow-hidden rounded-full bg-muted"
      >
        <div
          class="h-full rounded-full bg-primary motion-safe:transition-[width]"
          style:width="{progress.percent}%"
        ></div>
      </div>
    {/if}

    {#if children.length > 0}
      <ul class="flex flex-col gap-1.5" data-testid="subcards-list">
        {#each children as child (child.id)}
          {@const childTitle = child.title || untitled}
          {@const done = isCardCompleted(child, properties, statusProp)}
          {@const statusOpt = statusOptionOf(child, statusProp)}
          {@const nested = subcardProgress(child.id, hierarchy.childrenMap, properties, statusProp)}
          <li
            data-subcard={child.id}
            data-completed={done ? '' : undefined}
            class="flex items-center gap-2 rounded-lg border py-1 pr-1 pl-2.5"
          >
            {#if onOpenCard}
              <button
                type="button"
                data-subcard-open={child.id}
                aria-label={m.card_subcard_open({ title: childTitle })}
                class="flex min-h-7 min-w-0 flex-1 items-center gap-1.5 rounded-sm text-left underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-touch"
                onclick={() => {
                  onOpenCard(child.id);
                }}
              >
                <CardNumber number={child.number} />
                {#if child.fields.icon}
                  <span class="shrink-0" aria-hidden="true">{child.fields.icon}</span>
                {/if}
                <span class={['min-w-0 truncate', done && 'text-muted-foreground line-through']}>
                  {childTitle}
                </span>
              </button>
            {:else}
              <span class="flex min-h-7 min-w-0 flex-1 items-center gap-1.5">
                <CardNumber number={child.number} />
                {#if child.fields.icon}
                  <span class="shrink-0" aria-hidden="true">{child.fields.icon}</span>
                {/if}
                <span class={['min-w-0 truncate', done && 'text-muted-foreground line-through']}>
                  {childTitle}
                </span>
              </span>
            {/if}

            {#if nested.total > 0}
              <span
                class="flex shrink-0 items-center gap-1 text-xs text-muted-foreground"
                title={m.card_subcards_badge({ done: nested.done, total: nested.total })}
              >
                <ListTreeIcon class="size-3" aria-hidden="true" />
                {nested.done}/{nested.total}
              </span>
            {/if}

            {#if statusOpt}
              <span class="shrink-0">
                <OptionChip name={statusOpt.label} color={statusOpt.color} />
              </span>
            {:else if done}
              <span
                class="inline-flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground"
                title={m.card_subcard_completed()}
              >
                <CheckIcon class="size-3.5" aria-hidden="true" />
                <span class="sr-only">{m.card_subcard_completed()}</span>
              </span>
            {/if}

            {#if editable}
              <IconButton
                label={m.card_subcard_detach({ title: childTitle })}
                data-subcard-detach={child.id}
                onclick={() => void detachSubcard(child.id)}
              >
                <UnlinkIcon aria-hidden="true" />
              </IconButton>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if canAdd}
      <div class="flex items-center gap-2">
        <Input
          bind:value={newTitle}
          type="text"
          placeholder={m.card_subcard_new_placeholder()}
          aria-label={m.card_subcard_new_placeholder()}
          autocomplete="off"
          enterkeyhint="done"
          disabled={creating}
          data-testid="subcard-new-input"
          class="min-w-0 flex-1 roomy:h-touch"
          onkeydown={onNewKeyDown}
        />
        <Button
          variant="secondary"
          size="sm"
          disabled={creating || newTitle.trim() === ''}
          data-testid="subcard-new-submit"
          class="shrink-0 roomy:min-h-touch"
          onclick={() => void createSubcard()}
        >
          <PlusIcon data-icon="inline-start" aria-hidden="true" />
          {m.card_subcard_new_add()}
        </Button>
      </div>
    {/if}
  </section>
{/if}
