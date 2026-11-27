<!--
  The cards of one day, with their titles in full: what a day of the grid
  opens. The same content wherever it is put: under the small grid of a
  narrow box, or in a popover next to the cell (then with a button that
  closes it).

  A line of it is a CardRow: the card's title, time and days, the
  properties the view turned on, and its "…". Where dates cannot be changed
  here, a line under the heading says why. Last, for someone who may make
  cards there, "New card on …": the way to make a
  card on a day that every pointer and the keys reach.
-->
<script lang="ts">
  import PlusIcon from '@lucide/svelte/icons/plus';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import type { PropertyUser } from '$lib/components/properties/index.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import { displayDay, displayDayLong } from '$lib/logic/format.ts';
  import type { PropertyTemplate } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';

  import CardRow from './CardRow.svelte';
  import type { Placed } from './placed.ts';

  interface Props {
    date: CalendarDate;
    placed: Placed;
    today: CalendarDate;
    templates: readonly PropertyTemplate[];
    shownProperties: readonly string[];
    users: readonly PropertyUser[];
    cardHref: (cardId: string) => string;
    onOpenCard: (cardId: string) => void;
    board: BoardStore;
    /** Opens the picker of a day for a card; none where dates cannot be set. */
    onChangeDate?: (cardId: string, line: HTMLElement | undefined) => void;
    /**
     * Why no date can be changed on this calendar, where none can: it is
     * placed by when cards were made or changed.
     */
    fixed?: string;
    /** Makes a card on the day; none where no card can be made here. */
    oncreate?: (date: CalendarDate) => void;
    /** Given where the list can be closed: a popover. */
    onclose?: () => void;
  }

  let {
    date,
    placed,
    today,
    templates,
    shownProperties,
    users,
    cardHref,
    onOpenCard,
    board,
    onChangeDate,
    fixed,
    oncreate,
    onclose,
  }: Props = $props();

  const uid = $props.id();
  const cards = $derived(placed.on(date));
  const rows = $derived({
    placed,
    templates,
    shownProperties,
    users,
    cardHref,
    onOpenCard,
    board,
    onChangeDate,
  });
</script>

<section
  data-testid="day-list"
  data-date={date}
  aria-labelledby="{uid}-title"
  aria-describedby={fixed ? `${uid}-fixed` : undefined}
  class="flex min-h-0 flex-1 flex-col"
>
  <header class="flex shrink-0 items-center gap-2 py-1 pr-1 pl-4">
    <h3 id="{uid}-title" class="font-semibold">{displayDayLong(date, placed.locale)}</h3>
    {#if date === today}
      <span class="rounded-full bg-primary px-2 py-0.5 text-xs font-medium text-primary-foreground">
        {m.calendar_today()}
      </span>
    {/if}
    <div class="flex-1"></div>
    {#if onclose}
      <IconButton label={m.calendar_close_day()} data-testid="day-list-close" onclick={onclose}>
        <XIcon />
      </IconButton>
    {:else}
      <!-- Keeps the row as high as where there is a button. -->
      <div class="h-8 roomy:h-touch"></div>
    {/if}
  </header>
  {#if fixed}
    <p id="{uid}-fixed" data-testid="dates-fixed" class="px-4 pb-1 text-xs text-muted-foreground">
      {fixed}
    </p>
  {/if}

  <ul
    class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-1 pb-[max(0.5rem,var(--fab-room,0px))]"
  >
    {#each cards as card (card.id)}
      <CardRow {card} {date} {...rows} describedBy={fixed ? `${uid}-fixed` : undefined} />
    {:else}
      <li class="px-3 py-2 text-muted-foreground">{m.calendar_no_cards()}</li>
    {/each}
    {#if oncreate}
      {@const create = oncreate}
      <li>
        <button
          type="button"
          data-testid="day-new-card"
          class="flex min-h-10 w-full items-center gap-2 rounded-lg px-3 text-left text-muted-foreground outline-none hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-12"
          onclick={() => {
            create(date);
          }}
        >
          <PlusIcon class="size-4 shrink-0" aria-hidden="true" />
          <span class="truncate"
            >{m.calendar_new_card_on({ day: displayDay(date, placed.locale) })}</span
          >
        </button>
      </li>
    {/if}
  </ul>
</section>
