<!--
  The month in a box too narrow for bars: a day is
  its number, and under it only how much is on it. A card of one day is a
  dot (three at most, then "+N"); a card of several days is a thin line
  across them (two rows at most; a third counts as a dot). The titles are in
  the list of the picked day, which the calendar draws underneath.

  Every day is a button, whatever the pointer: seven across 360 pixels are
  51 wide, and 3.75rem high. They are one stop of the Tab key; the arrow keys
  move the picked day.
-->
<script lang="ts">
  import { inMonth, layWeek, type Month } from '$lib/logic/calendar.ts';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import { displayDayLong, displayDayNumber, displayWeekday } from '$lib/logic/format.ts';

  import { dayKeys } from './keys.ts';
  import type { Placed } from './placed.ts';

  interface Props {
    weeks: readonly CalendarDate[][];
    placed: Placed;
    month: Month;
    today: CalendarDate;
    selected: CalendarDate;
    onpick: (date: CalendarDate) => void;
    onmove: (by: number) => void;
  }

  let { weeks, placed, month, today, selected, onpick, onmove }: Props = $props();

  /** Rows of lines under the number. */
  const LINES = 2;
  const DOTS = 3;
  const DOT_SLOTS = [0, 1, 2];

  const locale = $derived(placed.locale);
  const ranges = $derived(placed.spans.filter((span) => span.first !== span.last));

  const rows = $derived(
    weeks.map((week) => {
      // What does not fit in the rows of lines joins the dots.
      const lines = layWeek(week, ranges).filter((segment) => segment.lane < LINES);
      const days = week.map((date, column) => {
        const lined = lines.filter(
          (line) => column >= line.column && column < line.column + line.length,
        ).length;
        return { date, loose: placed.count(date) - lined };
      });
      return { week, lines, days };
    }),
  );
</script>

<div data-testid="mini-month" class="shrink-0">
  <div aria-hidden="true" class="grid grid-cols-7 text-center text-xs text-muted-foreground">
    {#each weeks[0] ?? [] as date (date)}
      <div data-weekday class="py-1">{displayWeekday(date, locale)}</div>
    {/each}
  </div>

  {#each rows as row (row.week[0])}
    <div data-week={row.week[0]} class="relative grid grid-cols-7">
      {#each row.days as { date, loose } (date)}
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
            'relative flex h-15 min-w-0 flex-col items-center rounded-lg pt-1 outline-none focus-visible:ring-2 focus-visible:ring-ring',
            !inMonth(date, month) && 'text-muted-foreground',
            picked && 'bg-muted',
          ]}
          onclick={() => {
            onpick(date);
          }}
          onkeydown={(event) => {
            dayKeys(event, onmove);
          }}
        >
          <span
            class={[
              'flex size-7 items-center justify-center rounded-full tabular-nums',
              isToday && 'bg-primary font-semibold text-primary-foreground',
              picked && !isToday && 'font-semibold',
            ]}
          >
            {displayDayNumber(date, locale)}
          </span>
          <!-- Under the lines: what is on the day and has no line. -->
          <span data-dots class="mt-auto flex h-3 items-center gap-0.5 pb-1">
            {#each DOT_SLOTS.slice(0, loose) as slot (slot)}
              <span class="size-1 rounded-full bg-muted-foreground"></span>
            {/each}
            {#if loose > DOTS}
              <span class="text-[0.6875rem] leading-none text-muted-foreground">
                +{loose - DOTS}
              </span>
            {/if}
          </span>
        </button>
      {/each}

      <div
        aria-hidden="true"
        class="pointer-events-none absolute inset-x-0 top-8.5 grid grid-cols-7 gap-y-0.5"
      >
        {#each row.lines as line (line.id)}
          <span
            data-line={line.id}
            class={[
              'h-1 bg-foreground/40',
              line.fromBefore ? 'rounded-l-none' : 'ml-1.5 rounded-l-full',
              line.toAfter ? 'rounded-r-none' : 'mr-1.5 rounded-r-full',
            ]}
            style:grid-column="{line.column + 1} / span {line.length}"
            style:grid-row={line.lane + 1}
          ></span>
        {/each}
      </div>
    </div>
  {/each}
</div>
