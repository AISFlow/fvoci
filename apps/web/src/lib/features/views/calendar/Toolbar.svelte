<!--
  The row above the grid: back and on a month or a week, today, and the month or week in
  view. The arrows and "today" come first and the name of the month after them: in a wide
  window the middle of the screen then falls on the name or on empty room, not on a
  button. At the far end, the cards without a date and how many: the word where the pane
  has room, the icon and the count always. Between them, month or week: two buttons where
  the pane has room, one that turns to the other where it has not. The proposal puts the
  narrow one in the view's "…" menu; the view header may not reach into a view, so it
  stays in this row.
-->
<script lang="ts">
  import CalendarDaysIcon from '@lucide/svelte/icons/calendar-days';
  import CalendarOffIcon from '@lucide/svelte/icons/calendar-off';
  import CalendarRangeIcon from '@lucide/svelte/icons/calendar-range';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  import type { Scale } from './period.svelte.ts';

  interface Props {
    /** The name of the month or the week in view. */
    title: string;
    scale: Scale;
    onscale: (scale: Scale) => void;
    onstep: (by: number) => void;
    ontoday: () => void;
    /** How many cards have no date, whether their list is open, and what opens it. */
    undated: number;
    undatedOpen: boolean;
    onundated: () => void;
    /** "No date" itself, for the focus to come back to. Bindable. */
    undatedButton?: HTMLElement | null;
  }

  let {
    title,
    scale,
    onscale,
    onstep,
    ontoday,
    undated,
    undatedOpen,
    onundated,
    undatedButton = $bindable(null),
  }: Props = $props();

  const week = $derived(scale === 'week');
  const SCALES = [
    { id: 'month', label: () => m['calendar.month']() },
    { id: 'week', label: () => m['calendar.week']() },
  ] as const;
</script>

<div
  data-testid="calendar-toolbar"
  class="flex h-12 shrink-0 items-center gap-0.5 border-b px-1.5 @columns/pane:px-3"
>
  <IconButton
    label={week ? m.calendar_previous_week() : m.calendar_previous_month()}
    data-testid="calendar-previous"
    onclick={() => {
      onstep(-1);
    }}
  >
    <ChevronLeftIcon />
  </IconButton>
  <IconButton
    label={week ? m.calendar_next_week() : m.calendar_next_month()}
    data-testid="calendar-next"
    onclick={() => {
      onstep(1);
    }}
  >
    <ChevronRightIcon />
  </IconButton>
  <Button
    variant="outline"
    data-testid="calendar-today"
    class="roomy:h-touch roomy:px-3"
    onclick={ontoday}
  >
    {m.calendar_today()}
  </Button>
  <h2
    data-testid="calendar-period"
    aria-live="polite"
    class="ml-2 min-w-0 truncate font-semibold @columns/pane:text-lg"
  >
    {title}
  </h2>
  <div class="min-w-1 flex-1"></div>
  <div
    role="group"
    aria-label={m.calendar_scale()}
    data-testid="calendar-scale"
    class="mr-1 hidden shrink-0 rounded-md border p-0.5 @columns/pane:flex roomy:border-0 roomy:p-0"
  >
    {#each SCALES as choice (choice.id)}
      <Button
        variant={scale === choice.id ? 'secondary' : 'ghost'}
        size="sm"
        aria-pressed={scale === choice.id}
        data-scale={choice.id}
        class="h-7 px-2.5 roomy:h-touch roomy:px-3"
        onclick={() => {
          onscale(choice.id);
        }}
      >
        {choice.label()}
      </Button>
    {/each}
  </div>
  <IconButton
    label={week ? m.calendar_show_month() : m.calendar_show_week()}
    data-testid="calendar-scale-toggle"
    class="shrink-0 @columns/pane:hidden"
    onclick={() => {
      onscale(week ? 'month' : 'week');
    }}
  >
    {#if week}<CalendarDaysIcon />{:else}<CalendarRangeIcon />{/if}
  </IconButton>
  <Button
    bind:ref={undatedButton}
    variant="ghost"
    aria-label={m.calendar_undated_count({ count: undated })}
    aria-expanded={undatedOpen}
    data-testid="calendar-undated"
    class="shrink-0 gap-1.5 px-2 roomy:h-touch roomy:min-w-touch"
    onclick={onundated}
  >
    <CalendarOffIcon />
    <span class="hidden @columns/pane:inline">{m.calendar_undated()}</span>
    <span
      class="inline-flex h-4.5 min-w-4.5 items-center justify-center rounded-full bg-muted px-1 text-[0.6875rem] leading-none font-semibold ring-1 ring-input-border"
    >
      {undated}
    </span>
  </Button>
</div>
