<!--
  What a drag shows above the board: the card or the column under the
  pointer, and in words what letting go would do when that is not plain to
  see (the pointer is outside the board; the view is sorted).

  Everything here is fixed to the window and takes no presses. The card and
  the column are drawn once more, at the size they had when they were picked
  up; they follow the pointer by a translation only.
-->
<script lang="ts">
  import CardFace from '$lib/components/card-face/CardFace.svelte';
  import OptionChip from '$lib/components/option-chip/OptionChip.svelte';
  import type { SortableActive } from '$lib/drag/sortable.svelte.ts';
  import type { Point } from '$lib/logic/drop-target.ts';
  import type { BoardGroup } from '$lib/logic/group.ts';
  import type { Card } from '$lib/logic/wire.ts';

  import { badgesOf, type FaceContext } from './board-data.ts';

  /** As many cards as a dragged column shows: more would not fit a screen. */
  const COLUMN_CARDS = 8;

  interface Props {
    /** Words to show while they apply; empty for none. */
    notice: string;
    face: FaceContext;
    cardDrag: SortableActive | undefined;
    cardAt: Point | undefined;
    card: Card | undefined;
    columnDrag: SortableActive | undefined;
    columnAt: Point | undefined;
    column: BoardGroup | undefined;
    columnName: string;
    isOption: boolean;
  }

  let {
    notice,
    face,
    cardDrag,
    cardAt,
    card,
    columnDrag,
    columnAt,
    column,
    columnName,
    isOption,
  }: Props = $props();
</script>

{#snippet faceOf(shown: Card, count: number, lifted: boolean)}
  {@const badges = badgesOf(shown)}
  {@const subcards = face.hierarchy?.progressOf(shown.id)}
  {@const parentHint = face.hierarchy?.parentOf(shown)}
  <CardFace
    card={shown}
    templates={face.templates}
    shownProperties={face.shownProperties}
    hasBody={badges.hasBody}
    tasksTotal={badges.tasksTotal}
    tasksDone={badges.tasksDone}
    subcardsTotal={subcards?.total ?? 0}
    subcardsDone={subcards?.done ?? 0}
    {parentHint}
    locale={face.locale}
    regional={face.regional}
    users={face.users}
    {lifted}
    {count}
  />
{/snippet}

{#if notice}
  <!-- Above the finger, at the top of the page, clear of what the device keeps there. -->
  <div
    data-testid="drag-notice"
    class="pointer-events-none fixed inset-x-0 top-[calc(var(--safe-top)+0.5rem)] z-60 flex justify-center pr-[calc(var(--safe-right)+0.75rem)] pl-[calc(var(--safe-left)+0.75rem)]"
  >
    <span
      class="rounded-full bg-foreground px-3 py-1 text-center text-sm font-medium text-background shadow-lg"
    >
      {notice}
    </span>
  </div>
{/if}

{#if cardDrag?.mode === 'pointer' && cardAt && card}
  <div
    data-drag-overlay={card.id}
    aria-hidden="true"
    inert
    class={[
      'pointer-events-none fixed top-0 left-0 z-50 rotate-1 will-change-transform',
      cardDrag.outside && 'opacity-50',
    ]}
    style:width="{cardDrag.width}px"
    style:translate="{cardAt.x}px {cardAt.y}px"
  >
    {@render faceOf(card, cardDrag.itemIds.length, true)}
  </div>
{/if}

{#if columnDrag?.mode === 'pointer' && columnAt && column}
  <!-- The column under the pointer: its heading and as many cards as fit its height. -->
  <div
    data-drag-overlay={column.option.id}
    aria-hidden="true"
    inert
    class={[
      'pointer-events-none fixed top-0 left-0 z-50 rotate-1 overflow-hidden rounded-lg bg-muted shadow-lg ring-2 ring-ring will-change-transform',
      columnDrag.outside && 'opacity-50',
    ]}
    style:width="{columnDrag.width}px"
    style:height="{columnDrag.height}px"
    style:translate="{columnAt.x}px {columnAt.y}px"
  >
    <div class="flex min-h-10 items-center gap-2 pt-1 pl-3 roomy:min-h-12">
      {#if isOption && column.option.id !== ''}
        <OptionChip name={column.option.label} color={column.option.color} />
      {:else}
        <span class="truncate text-sm font-medium">{columnName}</span>
      {/if}
      <span class="text-xs text-muted-foreground">{column.cards.length}</span>
    </div>
    <div class="flex flex-col gap-2 p-2">
      {#each column.cards.slice(0, COLUMN_CARDS) as shown (shown.id)}
        {@render faceOf(shown, 1, false)}
      {/each}
    </div>
  </div>
{/if}
