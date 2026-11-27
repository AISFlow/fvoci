<!--
  The day a date condition compares with. A filter keeps a calendar day, not
  a moment: the same day for every reader, compared with the day each card's
  date falls on in the reader's time zone (lib/logic/filter.ts).

  Picking a day saves it and closes the calendar. "Today" is the reader's
  today, in the time zone of their account.
-->
<script lang="ts">
  import { parseDate, type DateValue as PickedDay } from '@internationalized/date';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';

  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { dateOfInstant, type CalendarDate } from '$lib/logic/dates.ts';
  import { displayCalendarDate } from '$lib/logic/format.ts';
  import type { Regional } from '$lib/logic/regional.ts';
  import { DYNAMIC_FILTER_TODAY, formatFilterDate, parseFilterDate } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Calendar, Day as CalendarDay } from '$lib/ui/calendar/index.js';

  interface Props {
    /** Names the field: "Value for Due date". */
    name: string;
    /** The stored value of the condition: a day as a filter writes it, or nothing. */
    value: string | undefined;
    locale: string;
    regional: Regional;
    presentation: Presentation;
    /** The current moment; defaults to the clock. Decides what "today" is. */
    now?: number;
    onChange: (value: string | undefined) => void;
  }

  let { name, value, locale, regional, presentation, now, onChange }: Props = $props();

  let open = $state(false);
  /** The month the calendar shows. */
  let month = $state<PickedDay | undefined>(undefined);
  let today = $state<CalendarDate | undefined>(undefined);

  /** The day the condition has now. */
  const isDynamicToday = $derived(value === DYNAMIC_FILTER_TODAY);
  const chosen = $derived(parseFilterDate(value, regional.timeZone, now));
  const text = $derived(
    isDynamicToday
      ? m.filter_today_dynamic()
      : chosen
        ? displayCalendarDate(chosen, {
            locale,
            timeZone: regional.timeZone,
            ...(now === undefined ? {} : { now }),
          })
        : '',
  );
  const weekStartsOn = $derived((regional.weekStart % 7) as 0 | 1 | 2 | 3 | 4 | 5 | 6);

  function setOpen(next: boolean): void {
    if (next) {
      today = dateOfInstant(now ?? Date.now(), regional.timeZone);
      month = parseDate(chosen ?? today);
    }
    open = next;
  }

  function pick(picked: CalendarDate | undefined): void {
    const next = picked === undefined ? undefined : formatFilterDate(picked);
    if (next !== value) onChange(next);
    open = false;
  }

  function pickDynamicToday(): void {
    if (value !== DYNAMIC_FILTER_TODAY) onChange(DYNAMIC_FILTER_TODAY);
    open = false;
  }

  // The calendar marks the day of the device as today; the reader's today is
  // the day of their account's time zone (the same as the date property does).
  const NOT_TODAY =
    '[&[data-today]:not([data-selected])]:bg-transparent [&[data-today]:not([data-selected])]:text-inherit';
  const TODAY =
    'font-semibold not-data-selected:bg-accent not-data-selected:text-accent-foreground';
</script>

<PickerSurface bind:open={() => open, setOpen} {presentation} title={name}>
  {#snippet trigger({ props }: { props: Record<string, unknown> })}
    <button
      {...props}
      type="button"
      data-slot="filter-date"
      class="flex h-8 w-full min-w-0 items-center justify-between gap-1 rounded-lg border border-input-border px-2.5 text-left text-sm hover:bg-muted aria-expanded:bg-muted roomy:h-touch roomy:text-base"
    >
      <span class="sr-only">{name}</span>
      {#if text !== ''}
        <span class="min-w-0 truncate">{text}</span>
      {:else}
        <span class="text-muted-foreground">{m['DateFilter.empty']()}</span>
      {/if}
      <ChevronDownIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
    </button>
  {/snippet}

  <div class="flex flex-col gap-1.5 p-1" data-slot="filter-date-picker">
    <Calendar
      type="single"
      bind:value={
        () => (chosen ? parseDate(chosen) : undefined),
        (picked: PickedDay | undefined) => {
          if (picked) pick(picked.toString() as CalendarDate);
        }
      }
      bind:placeholder={month}
      {locale}
      {weekStartsOn}
      fixedWeeks
      preventDeselect
      calendarLabel={name}
      class="mx-auto bg-transparent p-1 group-data-[presentation=sheet]/surface:[--cell-size:var(--spacing-touch)]"
    >
      {#snippet day({ day: cell }: { day: PickedDay })}
        {@const isToday = cell.toString() === today}
        <CalendarDay
          data-reader-today={isToday ? '' : undefined}
          class={isToday ? TODAY : NOT_TODAY}
        />
      {/snippet}
    </Calendar>
    <hr class="border-border" />
    <div class="flex flex-wrap justify-between gap-1">
      <div class="flex gap-1">
        <Button
          variant="ghost"
          class="group-data-[presentation=sheet]/surface:min-h-touch"
          onclick={() => {
            pick(today);
          }}
        >
          {m['DateRange.today']()}
        </Button>
        <Button
          variant={isDynamicToday ? 'secondary' : 'ghost'}
          data-testid="filter-date-always-today"
          class="group-data-[presentation=sheet]/surface:min-h-touch"
          onclick={pickDynamicToday}
        >
          {m.filter_today_dynamic()}
        </Button>
      </div>
      <Button
        variant="ghost"
        class="group-data-[presentation=sheet]/surface:min-h-touch"
        disabled={chosen === undefined && !isDynamicToday}
        onclick={() => {
          pick(undefined);
        }}
      >
        {m['DateRange.clear']()}
      </Button>
    </div>
  </div>
</PickerSurface>
