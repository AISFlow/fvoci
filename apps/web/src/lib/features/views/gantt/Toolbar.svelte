<!--
  The row above the chart: "Today" brings today into view, and two buttons
  zoom out and in a step (days, weeks, months), the step said between them
  where the pane has room.

  At the far end, how many relations are out of order (a card starts before
  a card it comes after has ended) and how many cards are on a loop, each
  only where there are any, with their mark, word and count (the word is
  not left to the colour and the mark on a narrow page: the row scrolls),
  and the cards without a date: the word, mark and count always. The first two are
  buttons that go to the next such card. Where no card with a bar is on a
  loop, the cards on one without a date are counted instead (a loop of
  them alone was shown nowhere): the button opens their list, which marks
  them. Where cards with a bar are counted, "No date" bears the mark of a
  loop and says how many of its cards are on one. One that has the
  focus when its count comes to nothing stays until the focus leaves it, and
  a change of the counts is said to a screen reader.

  The tools scroll sideways where the row is too narrow for all of them.
  "No date" stays beside them, so its count and way into the list remain in sight.
-->
<script lang="ts">
  import CalendarOffIcon from '@lucide/svelte/icons/calendar-off';
  import RepeatIcon from '@lucide/svelte/icons/repeat';
  import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
  import ZoomInIcon from '@lucide/svelte/icons/zoom-in';
  import ZoomOutIcon from '@lucide/svelte/icons/zoom-out';
  import type { Component } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { GANTT_ZOOMS } from '$lib/logic/gantt.ts';
  import type { GanttZoom } from '$lib/logic/view-members.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    ontoday: () => void;
    /** The view reads a card relation as its dependencies: there is something to count. */
    related: boolean;
    /** How many relations are out of order, and what goes to the next card of one. */
    late: number;
    onlate: () => void;
    /** How many cards with a bar are on a loop (else without a date), and what goes to the next. */
    loops: number;
    /** The loops counted are of cards without a date: the button shows their list. */
    loopsUndated?: boolean;
    onloop: () => void;
    zoom: GanttZoom;
    onzoom: (zoom: GanttZoom) => void;
    /** How many cards have no date, whether their list is open, and what opens it. */
    undated: number;
    /** Of those, how many are on a loop and not counted by "On a loop": "No date" says so. */
    undatedLoops?: number;
    undatedOpen: boolean;
    onundated: () => void;
    /** "No date" itself, for the focus to come back to. Bindable. */
    undatedButton?: HTMLElement | null;
    /** A card that is open and has no bar any more: "No date" stands for it when it closes. */
    undatedCard: string | undefined;
  }

  let {
    ontoday,
    related,
    late,
    onlate,
    loops,
    loopsUndated = false,
    onloop,
    zoom,
    onzoom,
    undated,
    undatedLoops = 0,
    undatedOpen,
    onundated,
    undatedButton = $bindable(null),
    undatedCard,
  }: Props = $props();

  const ZOOM_NAMES: Record<GanttZoom, () => string> = {
    day: m.gantt_zoom_day,
    week: m.gantt_zoom_week,
    month: m.gantt_zoom_month,
  };
  /** The count that has the focus: it stays in the row while it does, also at nothing. */
  let kept = $state<string | undefined>(undefined);

  /** The steps either side of the one in use: closer is earlier in the list. */
  const closer = $derived(GANTT_ZOOMS[GANTT_ZOOMS.indexOf(zoom) - 1]);
  const farther = $derived(GANTT_ZOOMS[GANTT_ZOOMS.indexOf(zoom) + 1]);
</script>

{#snippet problem(
  id: string,
  Mark: Component<{ class?: string }>,
  word: string,
  name: string,
  count: number,
  onclick: () => void,
)}
  <Button
    variant="ghost"
    aria-label={name}
    title={name}
    data-testid={id}
    class="shrink-0 gap-1.5 px-2 text-destructive roomy:h-touch roomy:min-w-touch"
    {onclick}
    onfocus={() => {
      kept = id;
    }}
    onblur={() => {
      kept = undefined;
    }}
  >
    <Mark />
    <span>{word}</span>
    <span
      data-count
      class="inline-flex h-4.5 min-w-4.5 items-center justify-center rounded-full px-1 text-[0.6875rem] leading-none font-semibold ring-1 ring-current"
    >
      {count}
    </span>
  </Button>
{/snippet}

<div
  data-testid="gantt-toolbar"
  class="flex h-12 shrink-0 items-center gap-1 border-b px-1.5 @columns/pane:px-3"
>
  <div
    data-testid="gantt-tools"
    class="-mx-1 flex h-full min-w-0 flex-1 [scrollbar-width:none] items-center gap-1 overflow-x-auto px-1"
  >
    <Button
      variant="outline"
      data-testid="gantt-today"
      class="roomy:h-touch roomy:px-3"
      onclick={ontoday}
    >
      {m.calendar_today()}
    </Button>
    <!-- At an end of the steps a button stays where it is and does nothing: the focus is not lost. -->
    <IconButton
      label={m.gantt_zoom_out()}
      data-testid="gantt-zoom-out"
      aria-disabled={farther === undefined}
      class="aria-disabled:opacity-40"
      onclick={() => {
        if (farther) onzoom(farther);
      }}
    >
      <ZoomOutIcon />
    </IconButton>
    <span
      data-testid="gantt-zoom"
      aria-live="polite"
      class="sr-only text-sm text-muted-foreground @columns/pane:not-sr-only"
    >
      <span class="sr-only">{m.gantt_zoom_step({ step: ZOOM_NAMES[zoom]() })}</span>
      <span aria-hidden="true">{ZOOM_NAMES[zoom]()}</span>
    </span>
    <IconButton
      label={m.gantt_zoom_in()}
      data-testid="gantt-zoom-in"
      aria-disabled={closer === undefined}
      class="aria-disabled:opacity-40"
      onclick={() => {
        if (closer) onzoom(closer);
      }}
    >
      <ZoomInIcon />
    </IconButton>
    <div class="min-w-1 flex-1"></div>
    <!-- Said when a count changes, also when it comes to nothing: the buttons come and go without a word. -->
    <span class="sr-only" aria-live="polite" data-testid="gantt-problems">
      {related ? m.gantt_problems({ late, loops }) : ''}
    </span>
    {#if late > 0 || kept === 'gantt-late'}
      {@render problem(
        'gantt-late',
        TriangleAlertIcon,
        m.gantt_late(),
        m.gantt_late_next({ count: late }),
        late,
        onlate,
      )}
    {/if}
    {#if loops > 0 || kept === 'gantt-loops'}
      {@render problem(
        'gantt-loops',
        RepeatIcon,
        m.gantt_loop(),
        loopsUndated
          ? m.gantt_loop_undated_next({ count: loops })
          : m.gantt_loop_next({ count: loops }),
        loops,
        onloop,
      )}
    {/if}
  </div>
  <Button
    bind:ref={undatedButton}
    variant="ghost"
    aria-label={undatedLoops > 0
      ? m.gantt_undated_loops({ count: undated, loops: undatedLoops })
      : m.calendar_undated_count({ count: undated })}
    aria-expanded={undatedOpen}
    data-testid="gantt-undated"
    data-card={undatedCard}
    class="shrink-0 gap-1.5 px-2 roomy:h-touch roomy:min-w-touch"
    onclick={onundated}
  >
    <CalendarOffIcon />
    <span>{m.calendar_undated()}</span>
    {#if undatedLoops > 0}
      <RepeatIcon data-mark="loop" class="text-destructive" />
    {/if}
    <span
      class="inline-flex h-4.5 min-w-4.5 items-center justify-center rounded-full bg-muted px-1 text-[0.6875rem] leading-none font-semibold ring-1 ring-input-border"
    >
      {undated}
    </span>
  </Button>
</div>
