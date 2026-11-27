<!--
  One card of the chart: its number and title at the left, which stay there
  while the days scroll, and its bar on its days.

  The bar is a button that opens the card. The bars are one stop of the Tab
  key, and the arrow keys walk them (`onkeys`); `current` is the one the stop
  is on. The title is a link to the card for a pointer: it is out of the
  order of the Tab key, which has the bar. A bar may be a few pixels wide:
  for a finger it takes presses from a little around it, and the title is
  the wide thing to press.

  For someone who may edit, the card's "…" (`menu`) lies over the end of
  the title while the row is pointed at or has the focus: "Change date" (a
  picker of days, as on the calendar), duplicate, delete. It is the way to
  move a bar with the keys: Shift+F10 or the menu key on the bar opens it.
  It is no stop of the Tab key, which stops on the bars only, and takes no
  room from the title on a narrow page.

  Where the reader may change the dates, the bar is dragged to other days
  and its ends by their grips (bar-drag.svelte.ts): small handles just
  outside each end, shown while the row is pointed at, and always for a
  finger. They lie beside the bar, not on it: the room around a bar that a
  finger may press (`roomy:after:`) then reaches up and down only, and the
  grips take the room at its sides. An end that is cut at the edge of the
  axis has no grip, and for a finger a bar narrower than GRIPS_MIN_WIDTH
  has none: a finger on it would mostly be on a grip.

  A card that starts before a card it comes after has ended carries a
  warning mark, and one on a loop a mark of going round: next to its title,
  where they stay in sight while the days scroll (next to the bar they would
  lie on the arrows that leave it). The marks are for the eye; the name of
  the bar says the same in words.
-->
<script lang="ts">
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import RepeatIcon from '@lucide/svelte/icons/repeat';
  import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

  import type { Snippet } from 'svelte';

  import CardNumber from '$lib/components/card-number/CardNumber.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BarBox } from '$lib/logic/gantt.ts';

  import type { CardLine } from './chart.svelte.ts';

  /** In rem. */
  const LABEL_MIN_WIDTH = 3;
  /** In rem: the narrowest bar that has grips for a finger. */
  const GRIPS_MIN_WIDTH = 1.5;

  interface Props {
    line: CardLine;
    /** The Tab key stops on this bar. */
    current: boolean;
    href: string;
    /** The card is open and was opened from the title: the focus comes back to the title. */
    viaTitle: boolean;
    /** The reader was sent to this bar: its ring shows however the focus came. */
    pointed: boolean;
    onopen: (cardId: string) => void;
    /** The title was pressed. */
    onopentitle: (cardId: string) => void;
    onkeys: (event: KeyboardEvent, cardId: string) => void;
    /** The bar took the focus: the stop of the Tab key is on it from now on. */
    onfocused: (cardId: string, bar: HTMLElement) => void;
    /** The bar can be dragged, and has grips at its ends. */
    draggable?: boolean;
    /** Expands or collapses sub-cards beneath this parent row. */
    ontoggle?: (cardId: string) => void;
    /** The card's "…", over the end of the title: given the card. */
    menu?: Snippet<[string]>;
    /** The bar is being dragged: where it is drawn meanwhile, its days in words, and where they are said. */
    dragged?: {
      /** The first day it would go to. */
      first: string;
      box: BarBox;
      days: string;
      label: () => { x: number; part: number } | undefined;
    };
  }

  let {
    line,
    current,
    href,
    viaTitle,
    pointed,
    onopen,
    onopentitle,
    onkeys,
    onfocused,
    draggable = false,
    ontoggle,
    menu,
    dragged,
  }: Props = $props();

  /** Shift+F10 or the menu key opens the card's menu; Left/Right collapses or expands sub-cards. */
  function keys(event: KeyboardEvent): void {
    if (
      line.hasChildren &&
      ontoggle &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.shiftKey
    ) {
      if (event.key === 'ArrowLeft' && !line.collapsed) {
        event.preventDefault();
        ontoggle(line.id);
        return;
      }
      if (event.key === 'ArrowRight' && line.collapsed) {
        event.preventDefault();
        ontoggle(line.id);
        return;
      }
    }
    const asked = event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey);
    if (!asked || !menu) {
      onkeys(event, line.id);
      return;
    }
    event.preventDefault();
    const row = (event.currentTarget as HTMLElement).closest('[data-row]');
    row?.querySelector<HTMLElement>('[data-card-menu]')?.click();
  }

  const bar = $derived(dragged?.box ?? line.bar);

  /** The bar is wide enough for a few letters of the title; a narrower one is a plain block. */
  const labelled = $derived(bar.width >= LABEL_MIN_WIDTH);

  function follow(event: MouseEvent): void {
    // A new tab or window is the browser's to open.
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
      return;
    event.preventDefault();
    (event.currentTarget as HTMLElement | null)?.focus({ preventScroll: true });
    onopentitle(line.id);
  }
</script>

{#snippet grip(end: 'start' | 'end')}
  <div
    data-grip={end}
    aria-hidden="true"
    class={[
      'absolute top-1/2 flex h-6 w-2 -translate-y-1/2 cursor-ew-resize items-center justify-center roomy:h-8 roomy:w-3',
      end === 'start' ? '-translate-x-full' : '',
      bar.width < GRIPS_MIN_WIDTH && 'roomy:hidden',
    ]}
    style:left="{end === 'start' ? bar.x : bar.x + bar.width}rem"
  >
    <span
      class="h-3.5 w-1 rounded-full bg-primary/70 opacity-0 group-hover/row:opacity-100 roomy:h-4 roomy:opacity-60"
    ></span>
  </div>
{/snippet}

<li
  data-row={line.id}
  data-level={line.showIndent ? line.level : undefined}
  class="group/row group/card relative z-1 flex h-(--gantt-row) border-b border-border/60 can-hover:hover:bg-muted/40"
>
  <div
    class="sticky left-0 z-10 flex w-(--gantt-title) shrink-0 items-stretch border-r bg-background"
  >
    {#if line.showIndent}
      <div class="flex shrink-0 items-center ps-1">
        {#if line.level > 0}
          <span class="shrink-0" style:width="{line.level * 0.875}rem" aria-hidden="true"></span>
        {/if}
        {#if line.hasChildren}
          <button
            type="button"
            data-hierarchy-toggle={line.id}
            tabindex={current ? 0 : -1}
            aria-expanded={!line.collapsed}
            aria-label={line.collapsed
              ? m.card_hierarchy_expand({ title: line.title })
              : m.card_hierarchy_collapse({ title: line.title })}
            class="inline-flex size-6 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring roomy:size-touch"
            onclick={() => {
              ontoggle?.(line.id);
            }}
          >
            {#if line.collapsed}
              <ChevronRightIcon class="size-3.5" aria-hidden="true" />
            {:else}
              <ChevronDownIcon class="size-3.5" aria-hidden="true" />
            {/if}
          </button>
        {:else}
          <span class="size-6 shrink-0 roomy:size-touch" aria-hidden="true"></span>
        {/if}
      </div>
    {/if}
    <a
      {href}
      tabindex="-1"
      data-row-title
      data-card-return={viaTitle ? line.id : undefined}
      class={[
        'flex min-w-0 flex-1 items-center outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset can-hover:hover:bg-muted',
        line.showIndent ? 'ps-1 pe-3' : 'px-3',
      ]}
      onclick={follow}
    >
      <CardNumber number={line.number} class="pe-1.5" />
      <span data-title class="truncate">{line.title}</span>
      {#if line.late || line.loop}
        <span
          aria-hidden="true"
          class="ml-auto flex shrink-0 items-center gap-0.5 pl-1 text-destructive"
        >
          {#if line.late}<TriangleAlertIcon data-mark="late" class="size-3.5" />{/if}
          {#if line.loop}<RepeatIcon data-mark="loop" class="size-3.5" />{/if}
        </span>
      {/if}
    </a>
    {#if menu}
      <div class="absolute inset-y-0 right-0 flex items-center">{@render menu(line.id)}</div>
    {/if}
  </div>
  <div class="relative min-w-(--gantt-width) flex-1">
    <button
      type="button"
      data-bar={line.id}
      data-card={viaTitle ? undefined : line.id}
      data-late={line.late ? '' : undefined}
      data-loop={line.loop ? '' : undefined}
      data-dragged={dragged ? '' : undefined}
      data-drag-first={dragged?.first}
      tabindex={current ? 0 : -1}
      aria-expanded={line.hasChildren ? !line.collapsed : undefined}
      aria-label={line.name}
      title={line.name}
      class={[
        'absolute top-1/2 flex h-6 -translate-y-1/2 items-center gap-1 rounded-sm bg-primary/80 text-left text-xs text-primary-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background data-dragged:z-5 data-dragged:bg-primary data-dragged:shadow-lg data-dragged:ring-2 data-dragged:ring-ring can-hover:hover:bg-primary roomy:h-8 roomy:after:absolute roomy:after:-inset-y-1.5',
        // The room at the sides is the grips', where there are grips for a finger.
        draggable && bar.width >= GRIPS_MIN_WIDTH
          ? 'roomy:after:inset-x-0'
          : 'roomy:after:-inset-x-3',
        labelled && 'px-1.5',
        pointed && 'focus:ring-2 focus:ring-ring focus:ring-offset-1 focus:ring-offset-background',
        bar.fromBefore && 'rounded-l-none',
        bar.toAfter && 'rounded-r-none',
      ]}
      style:left="{bar.x}rem"
      style:width="{bar.width}rem"
      onclick={() => {
        onopen(line.id);
      }}
      aria-keyshortcuts={menu ? 'Shift+F10' : undefined}
      onkeydown={keys}
      onfocus={(event) => {
        onfocused(line.id, event.currentTarget);
      }}
    >
      {#if bar.fromBefore && labelled}
        <ChevronLeftIcon aria-hidden="true" class="size-3 shrink-0 opacity-70" />
      {/if}
      {#if labelled}<span class="min-w-0 truncate">{line.title}</span>{/if}
      {#if bar.toAfter && labelled}
        <ChevronRightIcon aria-hidden="true" class="ml-auto size-3 shrink-0 opacity-70" />
      {/if}
    </button>
    {#if dragged}
      <!--
        The days it would go to, by the pointer (which is always in sight; an
        end of a long bar may not be): a day is a few pixels at a far zoom.
        In its own row: a row is drawn in a block that cuts off what goes
        above or below it.
      -->
      {@const label = dragged.label()}
      <span
        data-testid="gantt-drag-days"
        aria-hidden="true"
        class="pointer-events-none absolute top-1/2 z-5 rounded-sm bg-popover px-1.5 py-0.5 text-xs whitespace-nowrap text-popover-foreground shadow-md"
        style:left="{label?.x ?? bar.x + bar.width}rem"
        style:transform="translate({-(label?.part ?? 0) * 100}%, -50%)"
      >
        {dragged.days}
      </span>
    {/if}
    {#if draggable && !bar.fromBefore}{@render grip('start')}{/if}
    {#if draggable && !bar.toAfter}{@render grip('end')}{/if}
  </div>
</li>
