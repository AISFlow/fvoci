<!--
  The weeks of a month as a grid, each card a bar across its days. Drawn
  where the box of the calendar has room for it (34rem and up).

  A week is one CSS grid: seven columns, a row for the numbers of the days,
  one row a bar, and the rest. The cells lie under the bars and span every
  row. How many rows of bars a cell has room for is worked out from the
  height of the grid, every time it changes; a day with more cards than that
  says "+N" in its last row, and a bar that does not fit is hidden in its
  whole week.

  What can be pressed depends on the pointer, not on the width:

  - With a mouse, the number of a day opens the cards of the day, a bar
    opens its card, "+N" opens the cards of the day. A "+" at the top right
    of a cell, there while it is pointed at, makes a card on that day, and
    while the day's number has the focus. It is no stop of the Tab key and
    hidden from screen readers: the cards of the day end with "New card on
    …", which every reader reaches.
  - With a finger, the whole cell is one button that opens the cards of the
    day. A bar is a picture.

  A week (`week`) is one row as high as the box: every card of
  it is drawn, nothing says "+N", and a bar is two lines (its title, then its
  time or days), 2.75rem high, which a finger can press too.

  The days are one stop of the Tab key: the picked day takes the focus, and
  the arrow keys move it a day or a week (`onmove`).

  While a card is dragged (card-drag.svelte.ts) the days it would go to are
  marked (`drop`), and its bars are faint where they were.
-->
<script lang="ts">
  import PlusIcon from '@lucide/svelte/icons/plus';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { foldWeek, inMonth, layWeek, type Month } from '$lib/logic/calendar.ts';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import type { Days } from '$lib/logic/dependencies.ts';
  import type { Card, PropertyTemplate } from '$lib/logic/wire.ts';
  import {
    displayDay,
    displayDayLong,
    displayDayNumber,
    displayWeekday,
  } from '$lib/logic/format.ts';

  import { Room } from '$lib/layout/room.svelte.ts';

  import Bar from './Bar.svelte';
  import { dayKeys } from './keys.ts';
  import type { Placed } from './placed.ts';

  interface Props {
    weeks: readonly CalendarDate[][];
    placed: Placed;
    month: Month;
    today: CalendarDate;
    selected: CalendarDate;
    /** The main pointer is a finger. */
    finger: boolean;
    /** One week, every card in it, bars of two lines. */
    week?: boolean;
    /** The properties the view turned on that a card has: the second line of a week's bar. */
    shownOf?: (card: Card) => readonly PropertyTemplate[];
    cardHref: (cardId: string) => string;
    onOpenCard: (cardId: string) => void;
    /** A day was pressed: its cards are wanted. */
    onpick: (date: CalendarDate) => void;
    /** An arrow key moves the picked day by this many days. */
    onmove: (by: number) => void;
    /** Makes a card on a day; none where no card can be made here. */
    oncreate?: (date: CalendarDate) => void;
    /** The bars can be dragged: those that can be pressed have grips. */
    draggable?: boolean;
    /** The card being dragged, and the days it would go to. */
    dragging?: { id: string; days: Days | undefined };
  }

  let {
    weeks,
    placed,
    month,
    today,
    selected,
    finger,
    week = false,
    shownOf = () => [],
    cardHref,
    onOpenCard,
    onpick,
    onmove,
    oncreate,
    draggable = false,
    dragging,
  }: Props = $props();

  const drop = $derived(dragging?.days);

  const locale = $derived(placed.locale);

  /** The rows together: their height inside the room kept for the home indicator. */
  const rowsBox = new Room();
  const height = $derived(rowsBox.height);

  /** The size of 1rem in pixels, at the reader's text size. */
  function rem(count: number): number {
    return count * parseFloat(getComputedStyle(document.documentElement).fontSize || '16');
  }

  // The row of numbers is 1.75rem and a bar with its gap 1.5rem; a week is never lower than 5.5rem.
  /** The bars of each week, laid once. */
  const laid = $derived(weeks.map((days) => layWeek(days, placed.spans)));

  const lanes = $derived.by(() => {
    if (week) {
      // As many rows as the week's cards need: none is folded into "+N".
      return Math.max(1, ...(laid[0] ?? []).map((segment) => segment.lane + 1));
    }
    const row = Math.max(rem(5.5), height / Math.max(1, weeks.length));
    return Math.max(1, Math.floor((row - rem(1.75) - 2) / rem(1.5)));
  });

  const rows = $derived(
    weeks.map((days, at) => ({ week: days, ...foldWeek(laid[at] ?? [], lanes) })),
  );
  const barRow = $derived(week ? '2.875rem' : '1.5rem');
</script>

<div data-testid="month-grid" data-lanes={lanes} class="flex min-h-0 flex-1 flex-col">
  <!-- The names of the days. Every cell says its own date in full, so these are for the eye. -->
  <div aria-hidden="true" class="grid shrink-0 grid-cols-7 border-b text-xs text-muted-foreground">
    {#each weeks[0] ?? [] as date (date)}
      <div data-weekday class="truncate px-2 py-1.5">{displayWeekday(date, locale)}</div>
    {/each}
  </div>

  <div
    {@attach rowsBox.attach}
    class="grid min-h-0 flex-1 overflow-y-auto pb-(--safe-bottom)"
    style:grid-template-rows="repeat({weeks.length}, minmax(5.5rem, 1fr))"
  >
    {#each rows as row (row.week[0])}
      <div
        data-week={row.week[0]}
        class="relative grid grid-cols-7 border-b last:border-b-0"
        style:grid-template-rows="1.75rem repeat({lanes}, {barRow}) minmax(0, 1fr)"
      >
        {#each row.week as date, column (date)}
          {@const isToday = date === today}
          {@const picked = date === selected}
          {@const label = placed.dayName(date, displayDayLong(date, locale))}
          {@const dropped = drop !== undefined && date >= drop.first && date <= drop.last}
          {@const cell = [
            'relative row-span-full row-start-1 flex min-w-0 items-start border-r text-left last:border-r-0',
            !week && !inMonth(date, month) && 'bg-muted/50 text-muted-foreground',
            dropped && 'bg-primary/10 ring-2 ring-primary/60 ring-inset',
          ]}
          {#snippet number()}
            <span
              class={[
                'flex h-6 min-w-6 items-center justify-center rounded-full px-1 text-xs tabular-nums',
                isToday && 'bg-primary font-semibold text-primary-foreground',
                picked && !isToday && 'bg-muted font-semibold ring-1 ring-input-border',
              ]}
            >
              {displayDayNumber(date, locale)}
            </span>
          {/snippet}

          {#if finger}
            <button
              type="button"
              data-day={date}
              data-day-button
              data-drop={dropped ? '' : undefined}
              aria-label={label}
              aria-current={isToday ? 'date' : undefined}
              aria-haspopup="dialog"
              tabindex={picked ? 0 : -1}
              class={[
                cell,
                'p-0.5 outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset active:bg-muted',
              ]}
              style:grid-column={column + 1}
              onclick={() => {
                onpick(date);
              }}
              onkeydown={(event) => {
                dayKeys(event, onmove);
              }}
            >
              {@render number()}
            </button>
          {:else}
            <div
              data-day={date}
              data-drop={dropped ? '' : undefined}
              class={[cell, 'group/day p-0.5']}
              style:grid-column={column + 1}
            >
              <button
                type="button"
                data-day-button
                aria-label={label}
                aria-current={isToday ? 'date' : undefined}
                aria-haspopup="dialog"
                tabindex={picked ? 0 : -1}
                class="rounded-full outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
                onclick={() => {
                  onpick(date);
                }}
                onkeydown={(event) => {
                  dayKeys(event, onmove);
                }}
              >
                {@render number()}
              </button>
              {#if oncreate}
                {@const create = oncreate}
                <button
                  type="button"
                  tabindex={-1}
                  aria-hidden="true"
                  data-new-card={date}
                  aria-label={m.calendar_new_card_on({ day: displayDay(date, locale) })}
                  title={m.calendar_new_card_on({ day: displayDay(date, locale) })}
                  class="ml-auto flex size-6 items-center justify-center rounded-sm text-muted-foreground opacity-0 group-focus-within/day:opacity-100 group-hover/day:opacity-100 hover:bg-muted hover:text-foreground focus-visible:opacity-100"
                  onclick={() => {
                    create(date);
                  }}
                >
                  <PlusIcon class="size-4" aria-hidden="true" />
                </button>
              {/if}
            </div>
          {/if}

          {#if row.more[column]}
            {@const hidden = row.more[column] ?? 0}
            <div
              class={['z-10 min-w-0 px-0.5', finger && 'pointer-events-none']}
              style:grid-column={column + 1}
              style:grid-row={lanes + 1}
            >
              {#if finger}
                <!-- The cell's own name already says how many cards the day has. -->
                <span
                  data-more={date}
                  aria-hidden="true"
                  class="flex h-5.5 items-center px-1.5 text-xs text-muted-foreground"
                >
                  {m.calendar_more({ count: hidden })}
                </span>
              {:else}
                <button
                  type="button"
                  data-more={date}
                  aria-label={m.calendar_more_on({ day: displayDay(date, locale), count: hidden })}
                  aria-haspopup="dialog"
                  class="flex h-5.5 w-full items-center rounded-sm px-1.5 text-left text-xs text-muted-foreground hover:bg-muted hover:text-foreground"
                  onclick={() => {
                    onpick(date);
                  }}
                >
                  {m.calendar_more({ count: hidden })}
                </button>
              {/if}
            </div>
          {/if}
        {/each}

        {#each row.shown as segment (segment.id)}
          {@const card = placed.card(segment.id)}
          {#if card}
            <div
              class={[
                'z-10 min-w-0 self-start',
                segment.fromBefore ? 'pl-0' : 'pl-0.5',
                segment.toAfter ? 'pr-0' : 'pr-0.5',
                finger && !week && 'pointer-events-none',
              ]}
              style:grid-column="{segment.column + 1} / span {segment.length}"
              style:grid-row={segment.lane + 2}
            >
              <Bar
                cardId={card.id}
                title={placed.title(card)}
                number={card.number}
                name={placed.name(card)}
                time={segment.fromBefore ? undefined : placed.time(card)}
                fromBefore={segment.fromBefore}
                toAfter={segment.toAfter}
                pressable={!finger || week}
                tall={week}
                detail={week ? placed.detail(card, shownOf(card)) : undefined}
                href={cardHref(card.id)}
                onopen={onOpenCard}
                grips={draggable && (!finger || week)}
                dragged={dragging?.id === card.id}
              />
            </div>
          {/if}
        {/each}
      </div>
    {/each}
  </div>
</div>
