<!--
  The two rows of marks above the chart: months over days or weeks, years
  over months. They stay at the top while the rows scroll under them, and
  the corner stays at the left while the days scroll.

  For the eye only: every bar says its own days to a screen reader.

  The name of a month or a year stays in sight for as long as any of its
  days is: it sticks to the left edge of the days while its box scrolls out.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import type { Axis, Scale } from '$lib/logic/gantt.ts';

  import { lowerLabel, upperLabel } from './names.ts';

  interface Props {
    axis: Axis;
    scale: Scale;
    /** Where today is, in rem from the start of the axis: the middle of its day. */
    todayX: number;
    today: CalendarDate;
    locale: string;
  }

  let { axis, scale, todayX, today, locale }: Props = $props();

  /** A name of the upper row follows the scrolling only in a box with room to: in rem. */
  const NAME_ROOM = 8;
</script>

<div
  aria-hidden="true"
  data-testid="gantt-axis"
  class="sticky top-0 z-30 flex h-12 border-b bg-background text-xs text-muted-foreground"
>
  <div
    data-gantt-corner
    class="sticky left-0 z-10 flex w-(--gantt-title) shrink-0 items-end border-r bg-background px-3 pb-1.5 font-medium"
  >
    {m['TableComponent.name']()}
  </div>
  <div class="relative w-(--gantt-width) shrink-0">
    {#each scale.upper as tick (tick.date)}
      <div
        data-upper={tick.date}
        class={['absolute top-0 h-6 overflow-clip', tick.x > 0 && 'border-l']}
        style:left="{tick.x}rem"
        style:width="{tick.width}rem"
      >
        <!-- A box too narrow for a name (a month cut at an end of the axis) shortens it in place. -->
        <span
          class={[
            'px-2 py-1 font-medium text-foreground',
            tick.width >= NAME_ROOM
              ? 'sticky left-(--gantt-title) inline-block whitespace-nowrap'
              : 'block truncate',
          ]}
        >
          {upperLabel(axis.zoom, tick.date, locale)}
        </span>
      </div>
    {/each}
    {#each scale.lower as tick (tick.date)}
      <div
        data-tick={tick.date}
        class={[
          'absolute top-6 flex h-6 items-center overflow-clip border-t tabular-nums',
          tick.x > 0 && 'border-l',
          axis.zoom === 'day' ? 'justify-center' : 'px-1.5',
          axis.zoom === 'day' && tick.date === today && 'font-semibold text-foreground',
        ]}
        style:left="{tick.x}rem"
        style:width="{tick.width}rem"
      >
        {lowerLabel(axis.zoom, tick.date, locale)}
      </div>
    {/each}
    <!-- Where the line of today begins: inside the row, so that the corner covers it too. -->
    <div
      class="absolute bottom-0 h-1.5 w-2 -translate-x-1/2 rounded-t-full bg-primary"
      style:left="{todayX}rem"
    ></div>
  </div>
</div>
