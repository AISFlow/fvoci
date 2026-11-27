<!--
  One card as a line of a list: in the list of a day, or among the cards
  without a date. The line is a link that opens the card: its title, and
  where it is listed under a day (`date`), its time on its first day (the
  time is the start's) and for a card of several days which of them this
  is ("November 9 – 13 · Day 2 of 5"). Under it, the properties the view
  turned on. At its end, for someone who may edit, the card's
  "…" (CardMenu), whose first line changes its date.

  A card on a loop of a Gantt chart's relation says so (`loop`), in words
  next to the mark.

  A line is 2.5rem high for a mouse and 3rem for a finger.
-->
<script lang="ts">
  import RepeatIcon from '@lucide/svelte/icons/repeat';

  import { faceProperties } from '$lib/components/card-face/face.ts';
  import CardNumber from '$lib/components/card-number/CardNumber.svelte';
  import { PropertyValue, type PropertyUser } from '$lib/components/properties/index.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { daysBetween, type CalendarDate } from '$lib/logic/dates.ts';
  import type { Card, PropertyTemplate } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';

  import CardMenu from './CardMenu.svelte';
  import type { Placed } from './placed.ts';

  interface Props {
    card: Card;
    placed: Placed;
    /** The day the line is listed under; none among the cards without a date. */
    date?: CalendarDate;
    templates: readonly PropertyTemplate[];
    shownProperties: readonly string[];
    users: readonly PropertyUser[];
    cardHref: (cardId: string) => string;
    onOpenCard: (cardId: string) => void;
    board: BoardStore;
    /** Opens the picker of a day for a card, by this line; none where dates cannot be set. */
    onChangeDate?: (cardId: string, line: HTMLElement | undefined) => void;
    /** The id of what the card's "…" is described by: why its date cannot be changed here. */
    describedBy?: string;
    /** The card is on a loop of what comes before what. */
    loop?: boolean;
  }

  let {
    card,
    placed,
    date,
    templates,
    shownProperties,
    users,
    cardHref,
    onOpenCard,
    board,
    onChangeDate,
    describedBy,
    loop = false,
  }: Props = $props();

  const span = $derived(date ? placed.span(card.id) : undefined);
  const time = $derived(span && span.first === date ? placed.time(card) : undefined);
  const range = $derived(date ? placed.range(card) : undefined);
  /** "Day 2 of 5", for a card of several days. */
  const dayOf = $derived(
    span && date && span.first !== span.last
      ? m.calendar_day_of({
          nth: daysBetween(span.first, date) + 1,
          count: daysBetween(span.first, span.last) + 1,
        })
      : undefined,
  );
  const shown = $derived(faceProperties(card, templates, shownProperties));

  /** The line itself, for the picker to hang by. Not reactive. */
  let line: HTMLElement | undefined;

  function attachLine(element: HTMLElement): () => void {
    line = element;
    return () => {
      line = undefined;
    };
  }

  function open(event: MouseEvent): void {
    // A new tab or window is the browser's to open.
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
      return;
    event.preventDefault();
    onOpenCard(card.id);
  }
</script>

<li data-card={card.id} class="group/card rounded-lg hover:bg-muted" {@attach attachLine}>
  <div class="flex min-w-0 items-center pr-1">
    <a
      href={cardHref(card.id)}
      class="flex min-h-10 min-w-0 flex-1 flex-col justify-center rounded-lg px-3 py-1 outline-none focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-12"
      onclick={open}
    >
      <span class="flex min-w-0 gap-1.5">
        <CardNumber number={card.number} />
        <span data-title class={['truncate', !card.title && 'text-muted-foreground']}>
          {placed.title(card)}
        </span>
      </span>
      {#if loop}
        <span data-mark="loop" class="flex items-center gap-1 text-xs text-destructive">
          <RepeatIcon aria-hidden="true" class="size-3.5 shrink-0" />{m.gantt_loop()}
        </span>
      {/if}
      {#if time || range}
        <span class="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
          {#if time}<span class="shrink-0">{time}</span>{/if}
          {#if range}<span class="truncate">{range} · {dayOf}</span>{/if}
        </span>
      {/if}
    </a>
    {#if board.canEdit}
      <CardMenu
        {board}
        {card}
        title={placed.title(card)}
        dated={placed.span(card.id) !== undefined}
        onChangeDate={onChangeDate &&
          ((cardId: string) => {
            onChangeDate(cardId, line);
          })}
        {describedBy}
      />
    {/if}
  </div>
  {#if shown.length > 0}
    <div class="flex min-w-0 flex-wrap items-center gap-x-1 px-1 pb-1 text-xs">
      {#each shown as template (template.id)}
        <div data-property={template.id} title={template.name} class="min-w-0">
          <span class="sr-only">{template.name}:</span>
          <PropertyValue
            {template}
            {card}
            value={card.fields.properties[template.id]}
            mode="display"
            {users}
            locale={placed.locale}
            regional={placed.regional}
          />
        </div>
      {/each}
    </div>
  {/if}
</li>
