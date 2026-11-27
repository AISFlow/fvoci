<!--
  The value of a card relation property: other cards of the same board,
  picked by their titles. Each picked card is a chip with its title; pressing
  the chip opens that card.

  A card is shown only when it is in `cards` (the board's cards that are
  there). The identifier of a deleted card stays in the stored value, also
  when the value is changed, so that bringing the card back brings the
  relation back; it is not drawn. (The server takes it: it does not ask again
  for a card the value named before, whether it is in the trash or gone.)

  The card itself cannot be picked, a card is picked once, and no more than
  RELATION_LIMIT are named (a value that names more, written some other way,
  keeps them all until the reader takes some out). The hidden ones count, as
  the server counts them: when they fill the value, nothing more is offered,
  and a line says how many deleted cards hold places, with a button that
  takes them out. Nothing else leaves them out. A card template the value
  names is not among `cards` either: it counts as deleted here. The list shows the
  picked cards first, then the others by title, at most LIST_LIMIT at a
  time, so that a board of thousands of cards does not draw thousands of
  rows: typing narrows it.
-->
<script lang="ts">
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import RepeatIcon from '@lucide/svelte/icons/repeat';

  import PickList from '$lib/components/pick-list/PickList.svelte';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
  import { relationBadge } from '$lib/components/properties/relation-labels.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    updateRelationLink,
    type RelationKind,
    type RelationLink,
  } from '$lib/logic/relations.ts';
  import { foldCase } from '$lib/logic/text.ts';
  import { RELATION_LIMIT, relationLink, type Card } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { cn } from '$lib/utils.js';

  import RelationLinkItem from './RelationLinkItem.svelte';

  interface Props {
    /** The name of the property. */
    name: string;
    /** The card the value belongs to: it cannot name itself. */
    cardId: string;
    /** The relation links in the stored value, each card once. */
    links: readonly RelationLink[];
    /** The board's cards that are there, by identifier. */
    cards: ReadonlyMap<string, Card>;
    /** BCP 47 tag: the alphabetical order of the list. */
    locale: string;
    /**
     * Cards that may not be picked besides the card itself (they would close
     * a loop). A picked card among them is on a loop that is there already:
     * it is marked, and can be taken out.
     */
    excluded?: ReadonlySet<string>;
    editable: boolean;
    /** Shown where there is no value; empty for nothing. */
    placeholder: string;
    presentation?: Presentation;
    startOpen?: boolean;
    onChange?: (value: RelationLink[]) => void;
    /** Opens a card; without it the chips are only text. */
    onOpenCard?: (cardId: string) => void;
    /** See PropertyValueProps. */
    innerTabindex?: 0 | -1;
  }

  let {
    name,
    cardId,
    links,
    cards,
    locale,
    excluded,
    editable,
    placeholder,
    presentation = 'popover',
    startOpen = false,
    onChange,
    onOpenCard,
    innerTabindex,
  }: Props = $props();

  /** How many cards the list draws at a time. */
  const LIST_LIMIT = 50;

  // svelte-ignore state_referenced_locally (only where editing starts)
  let open = $state(startOpen);
  let search = $state('');
  /** The open editor, where the search is. */
  let editor: HTMLDivElement | undefined = $state();

  const untitled = (card: Card | undefined): string => card?.title || m['KanbanCard.untitled']();
  const titleOf = (id: string): string => untitled(cards.get(id));

  /** The picked links whose cards are there, each card once in order; never the card itself. */
  const shownLinks = $derived(
    links.filter(
      (link, at, all) =>
        link.card !== cardId &&
        cards.has(link.card) &&
        all.findIndex((other) => other.card === link.card) === at,
    ),
  );
  const shown = $derived(shownLinks.map((link) => link.card));
  /** The named cards that are not there (deleted), each once: written back as they are. */
  const hidden = $derived([
    ...new Set(
      links
        .filter((link) => link.card !== cardId && !cards.has(link.card))
        .map((link) => link.card),
    ),
  ]);
  const full = $derived(shown.length + hidden.length >= RELATION_LIMIT);

  /**
   * The cards that can be picked, by title, each with the text it is found
   * by. Worked out once per change of the cards, and only while the list is
   * open: nothing reads it otherwise.
   */
  const candidates = $derived.by(() => {
    const collator = new Intl.Collator(locale);
    return [...cards.values()]
      .filter((card) => card.id !== cardId)
      .map((card) => ({ card, title: untitled(card), folded: foldCase(untitled(card)) }))
      .sort(
        (first, second) =>
          collator.compare(first.title, second.title) || (first.card.id < second.card.id ? -1 : 1),
      );
  });

  /**
   * What the list offers for what was typed: the cards that can be added
   * first (Enter on the field adds the first), then the picked ones, which
   * can always be taken out. Once the value is full, only the picked ones.
   */
  const matches = $derived.by(() => {
    const wanted = foldCase(search.trim());
    const picked = new Set(shown);
    const found = candidates.filter(
      (entry) =>
        (wanted === '' || entry.folded.includes(wanted)) &&
        (picked.has(entry.card.id) || (!full && !excluded?.has(entry.card.id))),
    );
    return [
      ...found.filter((entry) => !picked.has(entry.card.id)),
      ...found.filter((entry) => picked.has(entry.card.id)),
    ].map((entry) => entry.card);
  });
  const listed = $derived(matches.slice(0, LIST_LIMIT));
  /** How many cards that match what was typed are not offered because they would close a loop. */
  const leftOut = $derived.by(() => {
    if (!excluded || full) return 0;
    const wanted = foldCase(search.trim());
    const picked = new Set(shown);
    return candidates.filter(
      (entry) =>
        excluded.has(entry.card.id) &&
        !picked.has(entry.card.id) &&
        (wanted === '' || entry.folded.includes(wanted)),
    ).length;
  });
  /** What the reader is told about the cards held back: deleted cards in the way, the limit, then loops. */
  /** Deleted cards fill the value, and taking them out leaves room. */
  const crowded = $derived(full && hidden.length > 0 && shown.length < RELATION_LIMIT);
  const heldBack = $derived(
    crowded
      ? m.property_relation_hidden_full({ count: hidden.length })
      : full
        ? m.property_relation_full({ count: RELATION_LIMIT })
        : leftOut > 0
          ? m.property_relation_loop({ count: leftOut })
          : '',
  );

  /**
   * The line about held-back cards goes into its live region only after the
   * region is on the page: one that appears holding its text is not read.
   */
  let regionReady = $state(false);
  const announceLater = (): (() => void) => {
    const frame = requestAnimationFrame(() => {
      regionReady = true;
    });
    return () => {
      cancelAnimationFrame(frame);
      regionReady = false;
    };
  };

  /** A picked card on a loop that is there already. */
  const onLoop = (id: string): boolean => excluded?.has(id) ?? false;

  /**
   * Writes the value: the picked cards that are there and the hidden ones
   * (unless the reader takes those out), in the order they were named, each
   * once, preserving existing `kind` and `lagDays` and never naming the card
   * itself. Newly picked cards default to finish-to-start with zero lag.
   */
  function write(next: readonly string[], keepHidden = true): void {
    const picked = new Set(next.filter((id) => id !== cardId && cards.has(id)));
    const hiddenSet = new Set(hidden);
    const keptIds = links
      .map((link) => link.card)
      .filter((id) => picked.has(id) || (keepHidden && hiddenSet.has(id)));
    const ordered = [...new Set([...keptIds, ...picked])];
    onChange?.(
      ordered.map((id) => {
        const existing = links.find((link) => link.card === id);
        return existing
          ? relationLink(existing.card, existing.kind, existing.lagDays)
          : relationLink(id);
      }),
    );
  }

  function updateLink(
    targetCardId: string,
    patch: { kind?: RelationKind; lagDays?: number },
  ): void {
    const cleaned = links.filter(
      (link, at, all) =>
        link.card !== cardId && all.findIndex((other) => other.card === link.card) === at,
    );
    onChange?.(updateRelationLink(cleaned, targetCardId, patch));
  }

  function pick(card: Card): void {
    if (shown.includes(card.id)) write(shown.filter((id) => id !== card.id));
    else if (!full && !excluded?.has(card.id)) write([...shown, card.id]);
  }

  const chipClass =
    'inline-flex max-w-full min-w-0 items-center gap-1 rounded-md bg-muted px-1.5 py-0.5 text-sm text-foreground';
</script>

{#snippet loopMark(id: string)}
  {#if onLoop(id)}
    <RepeatIcon
      class="size-3.5 shrink-0 text-destructive"
      aria-hidden="true"
      data-slot="relation-loop"
    />
  {/if}
{/snippet}

{#snippet chips()}
  {#each shownLinks as link (link.card)}
    {@const id = link.card}
    {@const badge = relationBadge(link.kind, link.lagDays)}
    {#if onOpenCard}
      <button
        type="button"
        data-slot="relation-chip"
        data-card-id={id}
        data-inner={innerTabindex === undefined ? undefined : ''}
        tabindex={innerTabindex}
        class={cn(chipClass, 'underline-offset-2 hover:underline focus-visible:underline')}
        data-loop={onLoop(id) ? '' : undefined}
        aria-label={m.property_relation_open({ title: titleOf(id) }) +
          (badge !== '' ? `, ${badge}` : '') +
          (onLoop(id) ? `, ${m.property_relation_in_loop()}` : '')}
        onclick={() => {
          onOpenCard(id);
        }}
      >
        <span class="truncate">{titleOf(id)}</span>
        {#if badge !== ''}
          <span
            data-slot="relation-badge"
            class="shrink-0 rounded-sm bg-background/70 px-1 text-[0.6875rem] font-medium text-muted-foreground tabular-nums"
          >
            {badge}
          </span>
        {/if}
        {@render loopMark(id)}
      </button>
    {:else}
      <span
        data-slot="relation-chip"
        data-card-id={id}
        data-loop={onLoop(id) ? '' : undefined}
        class={chipClass}
      >
        <span class="truncate">{titleOf(id)}</span>
        {#if badge !== ''}
          <span
            data-slot="relation-badge"
            class="shrink-0 rounded-sm bg-background/70 px-1 text-[0.6875rem] font-medium text-muted-foreground tabular-nums"
          >
            {badge}
          </span>
        {/if}
        {@render loopMark(id)}
        {#if onLoop(id)}<span class="sr-only">{m.property_relation_in_loop()}</span>{/if}
      </span>
    {/if}
  {/each}
{/snippet}

{#if editable}
  <div
    data-slot="property-relation"
    class="flex min-h-8 w-full min-w-0 flex-wrap items-center gap-1 roomy:min-h-touch"
  >
    {@render chips()}
    <PickerSurface
      bind:open
      {presentation}
      title={name}
      onOpenChange={(next) => {
        if (!next) search = '';
      }}
    >
      {#snippet trigger({ props }: { props: Record<string, unknown> })}
        <button
          {...props}
          type="button"
          data-slot="relation-edit"
          title={name}
          class={[
            'flex min-h-8 items-center rounded-md px-2 py-1 text-left text-muted-foreground hover:bg-muted aria-expanded:bg-muted roomy:min-h-touch',
            shown.length === 0 ? 'flex-1' : 'min-w-8 justify-center roomy:min-w-touch',
          ]}
        >
          <span class="sr-only">{name}</span>
          {#if shown.length > 0}
            <PencilIcon class="size-3.5" aria-hidden="true" />
          {:else if placeholder !== ''}
            <span data-slot="property-empty">{placeholder}</span>
          {/if}
        </button>
      {/snippet}

      <div class="flex flex-col gap-1" bind:this={editor}>
        {#if shownLinks.length > 0}
          <ul
            class="m-0 flex list-none flex-col gap-1 p-1"
            aria-label={m.property_relation_picked()}
          >
            {#each shownLinks as link (link.card)}
              <RelationLinkItem
                {link}
                title={titleOf(link.card)}
                inLoop={onLoop(link.card)}
                onUpdate={(patch: { kind?: RelationKind; lagDays?: number }) => {
                  updateLink(link.card, patch);
                }}
                onRemove={() => {
                  write(shown.filter((other) => other !== link.card));
                }}
              />
            {/each}
          </ul>
        {/if}
        <PickList
          bind:search
          items={listed}
          textOf={untitled}
          selected={shown}
          multiple
          label={name}
          placeholder={m.property_relation_search()}
          emptyText={full ? '' : m.property_no_matches()}
          onPick={pick}
          onBackspaceEmpty={() => {
            const last = shown.at(-1);
            if (last !== undefined) write(shown.filter((id) => id !== last));
          }}
        >
          {#snippet row(card: Card)}
            <span class="truncate">{untitled(card)}</span>
          {/snippet}
        </PickList>
        {#if matches.length > listed.length}
          <p class="px-2 py-1 text-sm text-muted-foreground" data-slot="relation-more">
            {m.property_relation_more({ count: matches.length - listed.length })}
          </p>
        {/if}
        <p
          class={['px-2 text-sm text-muted-foreground', regionReady && heldBack !== '' && 'py-1']}
          role="status"
          data-slot="relation-held-back"
          {@attach announceLater}
        >
          {regionReady ? heldBack : ''}
        </p>
        {#if crowded}
          <Button
            variant="ghost"
            class="justify-start group-data-[presentation=sheet]/surface:min-h-touch"
            data-slot="relation-drop-hidden"
            onclick={() => {
              write(shown, false);
              // The button goes with the deleted cards: the search keeps the focus.
              editor?.querySelector('input')?.focus();
            }}
          >
            {m.property_relation_drop_hidden({ count: hidden.length })}
          </Button>
        {/if}
        {#if shown.length > 0}
          <Button
            variant="ghost"
            class="justify-start group-data-[presentation=sheet]/surface:min-h-touch"
            onclick={() => {
              write([]);
            }}
          >
            {m.property_clear()}
          </Button>
        {/if}
      </div>
    </PickerSurface>
  </div>
{:else}
  <span
    data-slot="property-relation"
    class="flex min-h-8 min-w-0 flex-wrap items-center gap-1 px-2 py-1"
  >
    {@render chips()}
    {#if shown.length === 0 && placeholder !== ''}
      <span data-slot="property-empty" class="text-muted-foreground">{placeholder}</span>
    {/if}
  </span>
{/if}
