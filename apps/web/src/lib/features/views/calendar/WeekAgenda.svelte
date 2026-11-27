<!--
  A week in a box too narrow for bars: a strip of the seven days, and
  under it the cards of each day with their titles in full, a day with
  none saying so. A card of several days is listed under each of them
  ("Day 2 of 5").

  The days of the strip are one stop of the Tab key, as in the small grid;
  the arrow keys move the picked day, a day or a week. Picking a day brings
  its part of the list into view. Each day ends with "New card on …" where
  cards can be made there.
-->
<script lang="ts">
  import PlusIcon from '@lucide/svelte/icons/plus';
  import type { ComponentProps } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import {
    displayDay,
    displayDayLong,
    displayDayNumber,
    displayWeekday,
  } from '$lib/logic/format.ts';

  import CardRow from './CardRow.svelte';
  import { dayKeys } from './keys.ts';
  import type { Placed } from './placed.ts';

  /** What every line of the lists is given: the calendar's own. */
  type RowProps = Omit<ComponentProps<typeof CardRow>, 'card' | 'date'>;

  interface Props {
    week: readonly CalendarDate[];
    placed: Placed;
    today: CalendarDate;
    selected: CalendarDate;
    rows: RowProps;
    /** Why no date can be changed here, where none can. */
    fixed?: string;
    onpick: (date: CalendarDate) => void;
    onmove: (by: number) => void;
    oncreate?: (date: CalendarDate) => void;
  }

  let { week, placed, today, selected, rows, fixed, onpick, onmove, oncreate }: Props = $props();

  const uid = $props.id();

  const locale = $derived(placed.locale);
  /** The list of the days, for bringing one into view. Not reactive. */
  let list: HTMLElement | undefined;

  function attachList(element: HTMLElement): () => void {
    list = element;
    return () => {
      list = undefined;
    };
  }

  /** Brings the part of a day into view in the list, its heading at the top, when it is not. */
  function show(date: CalendarDate): void {
    const part = list?.querySelector<HTMLElement>(`[data-agenda-day="${date}"]`);
    if (!list || !part) return;
    const top = part.offsetTop;
    // Seen: its heading and its first lines, or all of it where it is shorter.
    const room = Math.min(part.offsetHeight, 160);
    const seen = top >= list.scrollTop && top + room <= list.scrollTop + list.clientHeight;
    if (!seen) list.scrollTop = top;
  }

  // The picked day stays in view: on the first draw, by the arrow keys, a week on or back.
  $effect(() => {
    show(selected);
  });
</script>

<div data-testid="week-agenda" class="flex min-h-0 flex-1 flex-col">
  <div data-testid="week-strip" class="grid shrink-0 grid-cols-7 border-b px-1 pb-1">
    {#each week as date (date)}
      {@const isToday = date === today}
      {@const picked = date === selected}
      <button
        type="button"
        data-day={date}
        data-day-button
        aria-label={placed.dayName(date, displayDayLong(date, locale))}
        aria-current={isToday ? 'date' : undefined}
        aria-pressed={picked}
        tabindex={picked ? 0 : -1}
        class={[
          'flex min-w-0 flex-col items-center gap-0.5 rounded-lg py-1 outline-none focus-visible:ring-2 focus-visible:ring-ring',
          picked && 'bg-muted',
        ]}
        onclick={() => {
          onpick(date);
        }}
        onkeydown={(event) => {
          dayKeys(event, onmove);
        }}
      >
        <span class="text-xs text-muted-foreground">{displayWeekday(date, locale)}</span>
        <span
          class={[
            'flex size-7 items-center justify-center rounded-full tabular-nums',
            isToday && 'bg-primary font-semibold text-primary-foreground',
          ]}
        >
          {displayDayNumber(date, locale)}
        </span>
        <span class={['size-1 rounded-full', placed.count(date) > 0 && 'bg-muted-foreground']}
        ></span>
      </button>
    {/each}
  </div>

  {#if fixed}
    <p id="{uid}-fixed" data-testid="dates-fixed" class="px-4 pt-1 text-xs text-muted-foreground">
      {fixed}
    </p>
  {/if}
  <div
    {@attach attachList}
    class="relative min-h-0 flex-1 overflow-y-auto overscroll-contain px-1 pb-[calc(max(0.5rem,var(--fab-room,0px))+var(--safe-bottom))]"
  >
    {#each week as date (date)}
      {@const cards = placed.on(date)}
      <!-- Not a section: seven named regions would be seven landmarks; the heading names the day. -->
      <div data-agenda-day={date} class="pt-2">
        <h3
          id="{uid}-{date}"
          class={[
            'flex items-center gap-2 px-3 py-1 text-sm font-semibold',
            date === selected ? 'text-foreground' : 'text-muted-foreground',
          ]}
        >
          {displayDayLong(date, locale)}
          {#if date === today}
            <span
              class="rounded-full bg-primary px-2 py-0.5 text-xs font-medium text-primary-foreground"
            >
              {m.calendar_today()}
            </span>
          {/if}
        </h3>
        <ul>
          {#each cards as card (card.id)}
            <CardRow {card} {date} {...rows} describedBy={fixed ? `${uid}-fixed` : undefined} />
          {:else}
            <li class="px-3 py-1 text-sm text-muted-foreground">{m.calendar_no_cards()}</li>
          {/each}
        </ul>
        {#if oncreate}
          {@const create = oncreate}
          <button
            type="button"
            data-new-card={date}
            class="flex min-h-10 w-full items-center gap-2 rounded-lg px-3 text-left text-sm text-muted-foreground outline-none hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-12"
            onclick={() => {
              create(date);
            }}
          >
            <PlusIcon class="size-4 shrink-0" aria-hidden="true" />
            <span class="truncate">{m.calendar_new_card_on({ day: displayDay(date, locale) })}</span
            >
          </button>
        {/if}
      </div>
    {/each}
  </div>
</div>
