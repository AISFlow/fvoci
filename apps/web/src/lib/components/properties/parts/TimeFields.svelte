<!--
  The time of day of a moment, on the clock of the reader's time zone: an
  hour and a minute to choose, and the half of the day where the reader's
  clock counts to twelve. What is shown is always read back from the moment,
  so a time the clocks skipped shows as the time it became.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    dayPeriods,
    fromTwelveHour,
    hourChoices,
    toTwelveHour,
    wallTimeOf,
  } from '$lib/logic/date-entry.ts';
  import type { HourCycle } from '$lib/logic/format.ts';

  interface Props {
    /** Names the group: "Time", "Start time", "End time". */
    label: string;
    instant: number;
    timeZone: string;
    hourCycle: HourCycle;
    locale: string;
    /** The hour (0 to 23) and minute the person chose. */
    onChange: (hour: number, minute: number) => void;
  }

  let { label, instant, timeZone, hourCycle, locale, onChange }: Props = $props();

  const wall = $derived(wallTimeOf(instant, timeZone));
  const twelve = $derived(toTwelveHour(wall.hour));
  const periods = $derived(dayPeriods(locale));
  const shownHour = $derived(hourCycle === 'h12' ? twelve.hour : wall.hour);
  const minutes = Array.from({ length: 60 }, (_, minute) => minute);
  const pad = (value: number): string => String(value).padStart(2, '0');

  /**
   * Makes a list show the stored time again. When the chosen time does not
   * exist and becomes the time already stored, nothing changes that would
   * move the list back by itself.
   */
  async function resync(select: HTMLSelectElement, shown: () => string): Promise<void> {
    await tick();
    select.value = shown();
  }

  function setHour(event: Event): void {
    const select = event.currentTarget as HTMLSelectElement;
    const hour = Number(select.value);
    onChange(hourCycle === 'h12' ? fromTwelveHour(hour, twelve.period) : hour, wall.minute);
    void resync(select, () => String(shownHour));
  }

  function setMinute(event: Event): void {
    const select = event.currentTarget as HTMLSelectElement;
    onChange(wall.hour, Number(select.value));
    void resync(select, () => String(wall.minute));
  }

  function setPeriod(event: Event): void {
    const select = event.currentTarget as HTMLSelectElement;
    onChange(fromTwelveHour(twelve.hour, select.value === 'pm' ? 'pm' : 'am'), wall.minute);
    void resync(select, () => twelve.period);
  }

  const selectClass =
    'h-8 rounded-lg border border-input-border bg-transparent px-1.5 text-base text-foreground outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 md:text-sm dark:bg-input/30 group-data-[presentation=sheet]/surface:h-touch group-data-[presentation=sheet]/surface:px-3';
</script>

{#snippet period()}
  <select
    class={selectClass}
    aria-label={m.property_date_period()}
    value={twelve.period}
    onchange={setPeriod}
  >
    <option value="am">{periods.am}</option>
    <option value="pm">{periods.pm}</option>
  </select>
{/snippet}

<div
  role="group"
  aria-label={label}
  data-slot="time-fields"
  class="flex min-h-8 items-center justify-between gap-2 px-2"
>
  <span class="text-sm">{label}</span>
  <span class="flex items-center gap-1">
    {#if hourCycle === 'h12' && periods.first}{@render period()}{/if}
    <select
      class={selectClass}
      aria-label={m.property_date_hour()}
      value={String(shownHour)}
      onchange={setHour}
    >
      {#each hourChoices(hourCycle) as hour (hour)}
        <option value={String(hour)}>{hourCycle === 'h12' ? hour : pad(hour)}</option>
      {/each}
    </select>
    <span aria-hidden="true">:</span>
    <select
      class={selectClass}
      aria-label={m.property_date_minute()}
      value={String(wall.minute)}
      onchange={setMinute}
    >
      {#each minutes as minute (minute)}
        <option value={String(minute)}>{pad(minute)}</option>
      {/each}
    </select>
    {#if hourCycle === 'h12' && !periods.first}{@render period()}{/if}
  </span>
</div>
