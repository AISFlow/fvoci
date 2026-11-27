<!--
  The calendar of a diary: a month whose written days are marked, by the face
  of their mood where they have one, else by a dot, each named for a screen
  reader too. Pressing a day picks it (its page, with its year open). It opens
  on the month of the day shown, read as it is opened (`DayHeader`), and
  reads each month it goes to after (`DiaryCalendar.showMonth`).

  Above it, the days written in a row up to today (or yesterday, while today
  is not written yet) and the most ever, counted on the reader's today.
-->
<script lang="ts">
  import { parseDate, type DateValue as PickedDay } from '@internationalized/date';
  import FlameIcon from '@lucide/svelte/icons/flame';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import type { Streak } from '$lib/logic/diary-streak.ts';
  import type { DiaryCalendar } from '$lib/state/index.ts';
  import { Calendar, Day } from '$lib/ui/calendar/index.js';

  import { MOOD_FACES } from './moods.ts';

  interface Props {
    calendar: DiaryCalendar;
    streak: Streak;
    /** The day the month opens on. */
    opening: CalendarDate;
    today: CalendarDate;
    locale: string;
    /** The first day of the week, 0 for Sunday. */
    weekStart: number;
    onPick: (date: CalendarDate) => void;
  }

  let { calendar, streak, opening, today, locale, weekStart, onPick }: Props = $props();
  const uid = $props.id();

  let placeholder = $derived<PickedDay>(parseDate(opening));
  const maxDay = $derived<PickedDay>(parseDate(today));

  function pick(picked: PickedDay | undefined): void {
    if (picked) onPick(picked.toString() as CalendarDate);
  }
</script>

<div class="flex flex-col gap-1" data-testid="diary-calendar">
  {#if calendar.counted}
    <p
      class="flex flex-wrap items-center gap-x-3 gap-y-1 px-2 pt-1 text-sm"
      data-testid="diary-streak"
    >
      <span class="flex items-center gap-1 font-medium" data-testid="streak-current">
        <FlameIcon class="size-4" />
        {m.diary_streak({ count: streak.current })}
      </span>
      <span class="text-muted-foreground" data-testid="streak-longest">
        {m.diary_streak_longest({ count: streak.longest })}
      </span>
    </p>
  {/if}
  <Calendar
    type="single"
    bind:value={() => undefined, pick}
    bind:placeholder={
      () => placeholder,
      (moved: PickedDay) => {
        // The placeholder follows the focus from day to day: a month is read as it is gone to.
        const another = moved.year !== placeholder.year || moved.month !== placeholder.month;
        placeholder = moved;
        if (another) void calendar.showMonth(moved.toString() as CalendarDate);
      }
    }
    {locale}
    weekStartsOn={(weekStart % 7) as 0 | 1 | 2 | 3 | 4 | 5 | 6}
    maxValue={maxDay}
    fixedWeeks
    calendarLabel={m.diary_pick_day()}
    class="bg-transparent p-1 [--cell-size:--spacing(9)] roomy:[--cell-size:var(--spacing-touch)]"
  >
    {#snippet day({ day, outsideMonth }: { day: PickedDay; outsideMonth: boolean })}
      {@const written = outsideMonth
        ? undefined
        : calendar.days.find((one) => one.date === day.toString())}
      {@const face = written?.mood ? MOOD_FACES[written.mood] : undefined}
      {@const said = `${uid}-${day.toString()}`}
      <!-- The day's name is its date (bits-ui's label): that it is written is its description. -->
      <Day
        class="gap-0.5 [&>span]:text-sm [&>span]:opacity-100"
        data-written={written ? '' : undefined}
        data-mood={written?.mood ?? undefined}
        aria-describedby={written ? said : undefined}
      >
        {#snippet children({ day: number }: { day: string })}
          <span class="text-sm leading-none">{number}</span>
          {#if face}
            <face.icon class="size-3.5" />
            <span id={said} class="sr-only">{m.diary_written_mood({ mood: face.name() })}</span>
          {:else if written}
            <span class="size-1.5 rounded-full bg-current" aria-hidden="true"></span>
            <span id={said} class="sr-only">{m.diary_written()}</span>
          {/if}
        {/snippet}
      </Day>
    {/snippet}
  </Calendar>
</div>
