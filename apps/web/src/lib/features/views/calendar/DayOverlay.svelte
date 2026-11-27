<!--
  The cards of the picked day over the grid of bars:
  a popover next to the day's cell, or a sheet on a compact page.

  Nothing opens it but the calendar (`open`); it has no trigger to give the
  focus back to. When the popover closes the focus goes to the day the list
  was of, unless the reader has put it somewhere else (a field, "No date").
  A card chosen in the list opens, or one is made, only once the popover has
  closed, with the focus on its day: that is where the card gives it back,
  unless it has a bar to go to (CardDialog, `data-card`). On a sheet the
  card opens at once; the sheet gives the focus back itself.
-->
<script lang="ts">
  import type { ComponentProps } from 'svelte';

  import BottomSheet from '$lib/components/sheet/BottomSheet.svelte';
  import { insets } from '$lib/layout/room.svelte.ts';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import { displayDayLong } from '$lib/logic/format.ts';
  import * as Popover from '$lib/ui/popover/index.js';

  import DayList from './DayList.svelte';

  type ListProps = Omit<
    ComponentProps<typeof DayList>,
    'date' | 'onOpenCard' | 'oncreate' | 'onclose'
  >;

  interface Props {
    open: boolean;
    sheet: boolean;
    /** The main pointer is a finger: the popover takes no focus ring. */
    finger: boolean;
    selected: CalendarDate;
    locale: string;
    /** What the list is given: the calendar's own. */
    lists: ListProps;
    /** Where the focus may go back to the day: "No date" has not taken it. */
    mayFocusDay: () => boolean;
    focusDay: () => void;
    onOpenCard: (cardId: string) => void;
    /** Makes a card on a day; none where no card can be made here. */
    oncreate?: (date: CalendarDate) => void;
  }

  let {
    open = $bindable(),
    sheet,
    finger,
    selected,
    locale,
    lists,
    mayFocusDay,
    focusDay,
    onOpenCard,
    oncreate,
  }: Props = $props();

  /** What a choice in the popover does once it has closed. Not reactive. */
  let afterClose: (() => void) | undefined;
  let popover = $state<HTMLElement | null>(null);

  const title = $derived(displayDayLong(selected, locale));

  /** The focus went somewhere else while the popover closed: a field, "No date". */
  function focusElsewhere(): boolean {
    const active = document.activeElement;
    return active instanceof HTMLElement && active !== document.body && !popover?.contains(active);
  }

  function later(then: () => void): void {
    open = false;
    if (sheet) then();
    else afterClose = then;
  }
</script>

{#if sheet}
  <BottomSheet bind:open {title}>
    <DayList
      date={selected}
      {...lists}
      onOpenCard={(cardId: string) => {
        later(() => {
          onOpenCard(cardId);
        });
      }}
      oncreate={oncreate &&
        ((date: CalendarDate) => {
          later(() => {
            oncreate(date);
          });
        })}
    />
  </BottomSheet>
{:else}
  <Popover.Root bind:open>
    <Popover.Content
      bind:ref={popover}
      customAnchor={`[data-testid="month-grid"] [data-day="${selected}"]`}
      side="right"
      align="start"
      collisionPadding={insets.collisionPadding}
      aria-label={title}
      data-testid="day-popover"
      class="max-h-[min(30rem,calc(100dvh-1rem))] w-80 max-w-[calc(100vw-1rem-var(--safe-left)-var(--safe-right))] gap-0 p-0 pb-1"
      onOpenAutoFocus={(event: Event) => {
        // A finger has not asked for the keyboard, and its ring is not wanted either.
        if (finger) event.preventDefault();
      }}
      onCloseAutoFocus={(event: Event) => {
        event.preventDefault();
        if (mayFocusDay() && !focusElsewhere()) focusDay();
        const then = afterClose;
        afterClose = undefined;
        then?.();
      }}
    >
      <DayList
        date={selected}
        {...lists}
        onOpenCard={(cardId: string) => {
          later(() => {
            onOpenCard(cardId);
          });
        }}
        oncreate={oncreate &&
          ((date: CalendarDate) => {
            later(() => {
              oncreate(date);
            });
          })}
        onclose={() => {
          open = false;
        }}
      />
    </Popover.Content>
  </Popover.Root>
{/if}
