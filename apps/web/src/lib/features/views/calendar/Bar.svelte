<!--
  One card in the grid: a bar across the days of one week it is on, with its
  number and title (and its time, when its value has one). An end that goes on into
  another week is cut square and marked with an arrow.

  In a week (`tall`) it is two lines, 2.75rem high: the title, then
  `detail` (its time, its days); a finger can press it there too.

  With a mouse the bar is a link that opens the card. With a finger it is a
  picture (`pressable` false): 22 pixels are too low to hit, and the cell
  under it takes the tap and opens the cards of the day.

  Where the reader may change the dates the bar is dragged to another day
  (card-drag.svelte.ts), and a bar that can be pressed has grips at the
  ends that are its own in this week, to make it longer or shorter: a few
  pixels at each end, shown while it is pointed at. While it is dragged it
  is faint where it was (`dragged`).
-->
<script lang="ts">
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';

  import CardNumber from '$lib/components/card-number/CardNumber.svelte';

  interface Props {
    cardId: string;
    title: string;
    /** The card's number on its board; none for a template. */
    number?: number;
    /** For a screen reader: the title, and the days when there are several. */
    name: string;
    /** The time of a value that has one, on the reader's clock. */
    time?: string;
    /** The card began before this week, or goes on after it. */
    fromBefore: boolean;
    toAfter: boolean;
    pressable: boolean;
    /** Two lines: a bar of a week. */
    tall?: boolean;
    /** The second line of a tall bar. */
    detail?: string;
    href: string;
    onopen: (cardId: string) => void;
    /** Grips at the ends of a bar that can be dragged. */
    grips?: boolean;
    /** The card is being dragged. */
    dragged?: boolean;
  }

  let {
    cardId,
    title,
    number,
    name,
    time,
    fromBefore,
    toAfter,
    pressable,
    tall = false,
    detail,
    href,
    onopen,
    grips = false,
    dragged = false,
  }: Props = $props();

  const shape = $derived([
    'relative flex min-w-0 items-center gap-1 rounded-sm bg-foreground/10 px-1.5 text-xs text-foreground',
    dragged && 'opacity-40',
    tall ? 'h-11' : 'h-5.5',
    fromBefore && 'rounded-l-none',
    toAfter && 'rounded-r-none',
  ]);

  function open(event: MouseEvent): void {
    // A new tab or window is the browser's to open.
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
      return;
    event.preventDefault();
    onopen(cardId);
  }
</script>

{#snippet grip(end: 'start' | 'end')}
  <span
    data-grip={end}
    aria-hidden="true"
    class={[
      'absolute inset-y-0 flex w-1.5 cursor-ew-resize items-center justify-center roomy:w-3',
      end === 'start' ? 'left-0' : 'right-0',
    ]}
  >
    <span
      class="h-3 w-0.5 rounded-full bg-foreground/50 opacity-0 group-hover/bar:opacity-100 roomy:opacity-60"
    ></span>
  </span>
{/snippet}

{#snippet content()}
  {#if grips && !fromBefore}{@render grip('start')}{/if}
  {#if grips && !toAfter}{@render grip('end')}{/if}
  {#if fromBefore}<ChevronLeftIcon aria-hidden="true" class="size-3 shrink-0 opacity-60" />{/if}
  {#if tall}
    <span class="flex min-w-0 flex-1 flex-col">
      <span class="flex min-w-0 gap-1 font-medium">
        <CardNumber {number} /><span class="truncate">{title}</span>
      </span>
      {#if detail}<span class="truncate text-muted-foreground">{detail}</span>{/if}
    </span>
  {:else}
    {#if time}<span class="shrink-0 text-muted-foreground">{time}</span>{/if}
    <CardNumber {number} />
    <span class="truncate">{title}</span>
  {/if}
  {#if toAfter}
    <ChevronRightIcon aria-hidden="true" class="ml-auto size-3 shrink-0 opacity-60" />
  {/if}
{/snippet}

{#if pressable}
  <a
    data-bar={cardId}
    data-card={cardId}
    {href}
    aria-label={name}
    class={[
      shape,
      'group/bar outline-none focus-visible:ring-2 focus-visible:ring-ring can-hover:hover:bg-foreground/15',
    ]}
    onclick={open}
  >
    {@render content()}
  </a>
{:else}
  <span data-bar={cardId} aria-hidden="true" class={shape}>
    {@render content()}
  </span>
{/if}
