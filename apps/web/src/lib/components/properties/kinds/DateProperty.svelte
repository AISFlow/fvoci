<!--
  The value of a date property: a day, or a moment when it carries a time,
  or a range from one to another.

  A value without a time is a calendar day, the same day for every reader.
  A value with a time is a moment; its day and its time are entered and shown
  on the clock of the reader's time zone, which comes from their account
  (`regional.timeZone`), not from the device. "Today" is the day the current
  moment falls on in that time zone.

  The picker works on a copy. Whatever closes it (Escape, pressing outside,
  the sheet's button) saves the copy, once, and only when it differs from
  the stored value. `onChange` gets the wire value that `formatDateValue`
  makes, or undefined for a cleared value.
-->
<script lang="ts">
  import { parseDate, type DateValue as PickedDay } from '@internationalized/date';
  import type { DateRange as PickedRange } from 'bits-ui';
  import { onDestroy, untrack, type Snippet } from 'svelte';

  import { isComposing } from '$lib/keys.ts';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    DEFAULT_HOUR,
    hasTime,
    instantOfWallTime,
    orderRange,
    parseTypedDate,
    typedDateText,
    withDay,
    withTime,
    withTimeOfDay,
    withoutTime,
  } from '$lib/logic/date-entry.ts';
  import {
    dateOfInstant,
    dateOfPoint,
    type CalendarDate,
    type DatePoint,
    type DateValue,
  } from '$lib/logic/dates.ts';
  import { displayCalendarDate, displayDateTime, type DisplayContext } from '$lib/logic/format.ts';
  import type { Regional } from '$lib/logic/regional.ts';
  import {
    formatDateValue,
    parseDateValue,
    type PropertyValue,
    type StoredDateValue,
  } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Calendar, Day as CalendarDay } from '$lib/ui/calendar/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Day as RangeCalendarDay, RangeCalendar } from '$lib/ui/range-calendar/index.js';
  import { Switch } from '$lib/ui/switch/index.js';

  import TimeFields from '../parts/TimeFields.svelte';

  interface Props {
    /** The name of the property. */
    name: string;
    /** The stored value. */
    value: PropertyValue | undefined;
    editable: boolean;
    /** Shown where there is no value; empty for nothing. */
    placeholder: string;
    locale: string;
    regional: Regional;
    /** The current moment; defaults to the clock. */
    now?: number;
    presentation?: Presentation;
    startOpen?: boolean;
    /**
     * Draws what opens the picker, in place of the value: a button of the bar
     * of a selection. `props` are spread on it, before its own attributes.
     */
    opener?: Snippet<[{ props: Record<string, unknown> }]>;
    /** Where the focus goes once the picker closes, for an `opener` not to be seen. */
    returnFocus?: () => HTMLElement | null | undefined;
    /**
     * The value stands for several cards whose values differ: none is shown
     * as chosen, and clearing is offered all the same (it clears them all).
     */
    mixed?: boolean;
    onChange?: (value: StoredDateValue | undefined) => void;
    /** More of the picker, under its switches (how a card repeats). */
    extra?: Snippet | undefined;
  }

  let {
    name,
    value,
    editable,
    placeholder,
    locale,
    regional,
    now,
    presentation = 'popover',
    startOpen = false,
    opener,
    returnFocus,
    mixed = false,
    onChange,
    extra,
  }: Props = $props();

  const zone = $derived(regional.timeZone);
  const stored = $derived(parseDateValue(value));
  const context = $derived<DisplayContext>({
    locale,
    timeZone: zone,
    hourCycle: regional.hourCycle,
    ...(now === undefined ? {} : { now }),
  });

  let open = $state(false);
  /** The copy the picker works on. */
  let draft = $state.raw<DateValue>({});
  // Whether the value is a range, and whether it carries a time, are choices
  // of their own: a range can be on before its end is picked, and the time
  // before there is a day.
  let rangeOn = $state(false);
  let timeOn = $state(false);
  let today = $state<CalendarDate | undefined>(undefined);
  /** The month the calendar shows. */
  let month = $state<PickedDay | undefined>(undefined);

  const fromDay = $derived(draft.from ? dateOfPoint(draft.from, zone) : undefined);
  const toDay = $derived(draft.to ? dateOfPoint(draft.to, zone) : undefined);
  const pickedDay = $derived(fromDay ? parseDate(fromDay) : undefined);
  const pickedRange = $derived({
    start: fromDay ? parseDate(fromDay) : undefined,
    end: toDay ? parseDate(toDay) : undefined,
  });

  // The typed dates follow the value; while a person types, the text is theirs.
  let fromText = $derived(fromDay ? typedDateText(fromDay, locale) : '');
  let toText = $derived(toDay ? typedDateText(toDay, locale) : '');

  /**
   * `open`, for the moment the component is removed. Not reactive on purpose:
   * while a component is being removed its state reads as it was before the
   * change that removes it, and closing the picker can be that change.
   */
  let picking = false;
  /** "Clear" was pressed over values that differ (`mixed`). */
  let clearedAll = false;

  // Removed while open (the card closed under it): keep what was picked.
  onDestroy(() => {
    if (picking) commit();
  });

  const pointText = (point: DatePoint): string =>
    point.instant === undefined
      ? displayCalendarDate(point.date, context)
      : displayDateTime(point.instant, context);

  const shown = $derived(open ? draft : stored);
  const text = $derived.by(() => {
    if (!shown.from) return '';
    return shown.to ? `${pointText(shown.from)} → ${pointText(shown.to)}` : pointText(shown.from);
  });

  function begin(): void {
    clearedAll = false;
    draft = stored;
    rangeOn = stored.to !== undefined;
    timeOn = hasTime(stored);
    today = dateOfInstant(now ?? Date.now(), zone);
    month = parseDate(stored.from ? dateOfPoint(stored.from, zone) : today);
  }

  function commit(): void {
    const next = formatDateValue(draft);
    const before = formatDateValue(stored);
    // Cleared over values that differ: nothing shown changed, but every card's value goes.
    const cleared = mixed && clearedAll && next === undefined;
    if (cleared || next?.from !== before?.from || next?.to !== before?.to) onChange?.(next);
  }

  function setOpen(next: boolean): void {
    if (next === open) return;
    if (next) {
      begin();
    } else {
      // A date still being typed counts: closing settles it as leaving the field does.
      typeFrom();
      if (rangeOn) typeTo();
      commit();
    }
    open = next;
    picking = next;
  }

  if (untrack(() => startOpen && editable)) {
    begin();
    open = true;
    picking = true;
  }

  /** A new end of the value on a day: nine in the morning when the value carries a time. */
  const pointOn = (day: CalendarDate): DatePoint =>
    timeOn
      ? { instant: instantOfWallTime({ date: day, hour: DEFAULT_HOUR, minute: 0 }, zone) }
      : { date: day };

  const startOn = (day: CalendarDate): DatePoint =>
    draft.from ? withDay(draft.from, day, zone) : pointOn(day);

  function pickDay(day: CalendarDate): void {
    draft = { from: startOn(day) };
  }

  function pickRange(start: CalendarDate, end: CalendarDate | undefined): void {
    const from = startOn(start);
    if (end === undefined) {
      // A new range has begun; its end is the next day picked.
      draft = { from };
      return;
    }
    // The end takes the time of day the old end had, or the start's.
    draft = orderRange({ from, to: withDay(draft.to ?? from, end, zone) }, 'from');
  }

  function pickToday(): void {
    if (today === undefined) return;
    if (rangeOn && fromDay !== undefined && toDay === undefined) {
      pickRange(fromDay <= today ? fromDay : today, fromDay <= today ? today : fromDay);
    } else {
      pickDay(today);
    }
    month = parseDate(today);
  }

  function typeFrom(): void {
    const day = parseTypedDate(fromText, locale);
    if (day !== undefined && day !== fromDay) {
      draft = orderRange({ ...draft, from: startOn(day) }, 'from');
      month = parseDate(day);
    }
    // What was typed becomes the date as the language writes it, or goes back to what it was.
    const settled = day ?? fromDay;
    fromText = settled ? typedDateText(settled, locale) : '';
  }

  function typeTo(): void {
    const day = parseTypedDate(toText, locale);
    if (day !== undefined && day !== toDay) {
      draft = draft.from
        ? orderRange({ from: draft.from, to: withDay(draft.to ?? draft.from, day, zone) }, 'to')
        : { from: pointOn(day) };
      month = parseDate(day);
    }
    const settled = day ?? toDay;
    toText = settled ? typedDateText(settled, locale) : '';
  }

  const canonical = (day: CalendarDate | undefined): string =>
    day ? typedDateText(day, locale) : '';

  /** Enter settles a typed date; Escape puts back the date it was, when it was changed. */
  function typedKeys(event: KeyboardEvent, settle: () => void, changed: boolean): void {
    if (isComposing(event)) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      settle();
    } else if (event.key === 'Escape' && changed) {
      // The first Escape puts the date back; the next one closes the picker.
      event.preventDefault();
      event.stopPropagation();
      fromText = canonical(fromDay);
      toText = canonical(toDay);
    }
  }

  function setTime(end: 'from' | 'to', hour: number, minute: number): void {
    const point = draft[end];
    if (point?.instant === undefined) return;
    const moved = { instant: withTimeOfDay(point.instant, hour, minute, zone) };
    draft = orderRange(end === 'from' ? { ...draft, from: moved } : { ...draft, to: moved }, end);
  }

  function toggleRange(on: boolean): void {
    rangeOn = on;
    if (!on && draft.from) draft = { from: draft.from };
  }

  function toggleTime(on: boolean): void {
    timeOn = on;
    draft = on ? withTime(draft, zone) : withoutTime(draft, zone);
  }

  function clear(): void {
    clearedAll = mixed;
    draft = {};
    rangeOn = false;
    timeOn = false;
  }

  // The calendar marks the day of the device as today. The reader's today is
  // the day of their account's time zone, so the calendar's own mark is
  // undone and the reader's day is marked instead.
  const NOT_TODAY =
    '[&[data-today]:not([data-selected])]:bg-transparent [&[data-today]:not([data-selected])]:text-inherit';
  const TODAY =
    'font-semibold not-data-selected:bg-accent not-data-selected:text-accent-foreground';
  const calendarClass =
    'mx-auto bg-transparent p-1 group-data-[presentation=sheet]/surface:[--cell-size:var(--spacing-touch)]';
  const weekStartsOn = $derived((regional.weekStart % 7) as 0 | 1 | 2 | 3 | 4 | 5 | 6);
  const rowClass =
    'flex min-h-8 items-center justify-between gap-2 px-2 text-sm group-data-[presentation=sheet]/surface:min-h-touch';
</script>

{#if editable}
  <PickerSurface bind:open={() => open, setOpen} {presentation} title={name} {returnFocus}>
    {#snippet trigger({ props }: { props: Record<string, unknown> })}
      {#if opener}
        {@render opener({ props })}
      {:else}
        <button
          {...props}
          type="button"
          data-slot="property-date"
          class="flex min-h-8 w-full min-w-0 items-center rounded-md px-2 py-1 text-left hover:bg-muted aria-expanded:bg-muted roomy:min-h-touch"
        >
          <span class="sr-only">{name}</span>
          {#if text !== ''}
            <span class="min-w-0 truncate">{text}</span>
          {:else if placeholder !== ''}
            <span data-slot="property-empty" class="text-muted-foreground">{placeholder}</span>
          {/if}
        </button>
      {/if}
    {/snippet}

    <div class="flex flex-col gap-1.5 p-1" data-slot="date-picker">
      <div class="flex gap-1.5">
        <Input
          bind:value={fromText}
          aria-label={rangeOn ? m.property_date_start() : name}
          placeholder={typedDateText('2000-12-31' as CalendarDate, locale)}
          inputmode="numeric"
          autocomplete="off"
          enterkeyhint="done"
          class="group-data-[presentation=sheet]/surface:h-touch"
          onblur={typeFrom}
          onkeydown={(event: KeyboardEvent) => {
            typedKeys(event, typeFrom, fromText !== canonical(fromDay));
          }}
        />
        {#if rangeOn}
          <Input
            bind:value={toText}
            aria-label={m['DateRange.endDate']()}
            placeholder={typedDateText('2000-12-31' as CalendarDate, locale)}
            inputmode="numeric"
            autocomplete="off"
            enterkeyhint="done"
            class="group-data-[presentation=sheet]/surface:h-touch"
            onblur={typeTo}
            onkeydown={(event: KeyboardEvent) => {
              typedKeys(event, typeTo, toText !== canonical(toDay));
            }}
          />
        {/if}
      </div>

      {#if rangeOn}
        <RangeCalendar
          bind:value={
            () => pickedRange,
            (range: PickedRange | undefined) => {
              if (range?.start) {
                pickRange(
                  range.start.toString() as CalendarDate,
                  range.end?.toString() as CalendarDate | undefined,
                );
              }
            }
          }
          bind:placeholder={month}
          {locale}
          {weekStartsOn}
          fixedWeeks
          calendarLabel={name}
          class={calendarClass}
        >
          {#snippet day({ day: cell }: { day: PickedDay })}
            {@const isToday = cell.toString() === today}
            <RangeCalendarDay
              data-reader-today={isToday ? '' : undefined}
              class={isToday ? TODAY : NOT_TODAY}
            />
          {/snippet}
        </RangeCalendar>
      {:else}
        <Calendar
          type="single"
          bind:value={
            () => pickedDay,
            (picked: PickedDay | undefined) => {
              if (picked) pickDay(picked.toString() as CalendarDate);
            }
          }
          bind:placeholder={month}
          {locale}
          {weekStartsOn}
          fixedWeeks
          preventDeselect
          calendarLabel={name}
          class={calendarClass}
        >
          {#snippet day({ day: cell }: { day: PickedDay })}
            {@const isToday = cell.toString() === today}
            <CalendarDay
              data-reader-today={isToday ? '' : undefined}
              class={isToday ? TODAY : NOT_TODAY}
            />
          {/snippet}
        </Calendar>
      {/if}

      {#if timeOn && draft.from?.instant !== undefined}
        <TimeFields
          label={rangeOn ? m.property_date_start_time() : m.property_date_time()}
          instant={draft.from.instant}
          timeZone={zone}
          hourCycle={regional.hourCycle}
          {locale}
          onChange={(hour: number, minute: number) => {
            setTime('from', hour, minute);
          }}
        />
        {#if rangeOn && draft.to?.instant !== undefined}
          <TimeFields
            label={m.property_date_end_time()}
            instant={draft.to.instant}
            timeZone={zone}
            hourCycle={regional.hourCycle}
            {locale}
            onChange={(hour: number, minute: number) => {
              setTime('to', hour, minute);
            }}
          />
        {/if}
      {/if}
      {#if timeOn}
        <p class="px-2 text-xs text-muted-foreground" data-slot="time-zone">
          {m.property_date_time_zone({ timeZone: zone })}
        </p>
      {/if}

      <hr class="border-border" />
      <label class={rowClass}>
        <span>{m['DateRange.endDate']()}</span>
        <Switch aria-label={m['DateRange.endDate']()} bind:checked={() => rangeOn, toggleRange} />
      </label>
      <label class={rowClass}>
        <span>{m.property_date_include_time()}</span>
        <Switch
          aria-label={m.property_date_include_time()}
          bind:checked={() => timeOn, toggleTime}
        />
      </label>
      {#if extra}
        <hr class="border-border" />
        {@render extra()}
      {/if}
      <hr class="border-border" />
      <div class="flex justify-between gap-1">
        <Button
          variant="ghost"
          class="group-data-[presentation=sheet]/surface:min-h-touch"
          onclick={pickToday}
        >
          {m['DateRange.today']()}
        </Button>
        <Button
          variant="ghost"
          class="group-data-[presentation=sheet]/surface:min-h-touch"
          disabled={!draft.from && !mixed}
          onclick={clear}
        >
          {m['DateRange.clear']()}
        </Button>
      </div>
    </div>
  </PickerSurface>
{:else if text !== ''}
  <span data-slot="property-date" class="block min-w-0 truncate px-2 py-1.5" title={text}>
    {text}
  </span>
{:else if placeholder !== ''}
  <span data-slot="property-empty" class="block px-2 py-1.5 text-muted-foreground">
    {placeholder}
  </span>
{/if}
