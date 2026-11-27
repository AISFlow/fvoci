<!--
  The calendar view: the cards of the view placed on the days of a month or
  of a week by one date property (`displayProperty`: the one the view
  names, or else the first kind of date of the board, or else when the card
  was made).

  Above, a row to go back and on a month or a week, to today, and between
  month and week. Under it, a box whose width decides what is drawn:

  - 34rem and wider: the weeks of the month as a grid of bars, or one week
    as high as the box with every card in it. A day opens its cards in a
    popover next to its cell, or on a sheet on a compact page.
  - narrower: a small grid of numbers, dots and lines over the list of the
    picked day, or the strip of a week over the lists of its
    seven days (WeekAgenda).

  A card is made on a day from "New card on …" at the end of the day's list,
  or with a mouse from the "+" of its cell; the
  header's "New" makes it on the day picked.
  A card's date is changed from the "…" of its line in a list: a picker of
  days (DayPicker) moves it whole. Where the cards are
  placed by a date nobody sets, no line offers it and the lists say why.
  With a pointer, a bar of the grid is dragged to another day, its ends to
  make it longer or shorter, and a card without a date from its column
  onto a day (card-drag.svelte.ts).

  Month or week, the month in view and the day picked are kept above the
  parts that draw them (Period): a window that changes size swaps the grid of
  bars for the small grid, and must not take them along. Month or week is
  remembered on the device for each view; the month and the day are the
  screen's only, and a new load starts at today.
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';

  import { faceProperties } from '$lib/components/card-face/face.ts';
  import BottomSheet from '$lib/components/sheet/BottomSheet.svelte';
  import { focusAdrift } from '$lib/focus.ts';
  import { overlayPresentation } from '$lib/layout/presentation.ts';
  import { fingerPointer, page, PaneRoom } from '$lib/layout/room.svelte.ts';
  import {
    datesWritable,
    displayProperty,
    weekOf,
    weeksOfMonth,
    type BarGrip,
  } from '$lib/logic/calendar.ts';
  import { addDays, dateOfInstant, type CalendarDate } from '$lib/logic/dates.ts';
  import type { Days } from '$lib/logic/dependencies.ts';
  import { displayDay } from '$lib/logic/format.ts';
  import type { Card } from '$lib/logic/wire.ts';
  import { registerNewCard } from '$lib/state/index.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';

  import type { ViewProps } from '../props.ts';

  import DayList from './DayList.svelte';
  import DayOverlay from './DayOverlay.svelte';
  import DayPicker from './DayPicker.svelte';
  import { CardDrag } from './card-drag.svelte.ts';
  import { CardMaker } from './making.svelte.ts';
  import MiniMonth from './MiniMonth.svelte';
  import MonthGrid from './MonthGrid.svelte';
  import { periodTitle } from './period-title.ts';
  import { Period, type Scale } from './period.svelte.ts';
  import { MinuteClock } from './clock.svelte.ts';
  import { Placed } from './placed.ts';
  import Toolbar from './Toolbar.svelte';
  import Undated from './Undated.svelte';
  import { UndatedColumn } from './undated-column.svelte.ts';
  import WeekAgenda from './WeekAgenda.svelte';
  import { writeDays } from './write.ts';

  let { board, view, cards, cardHref, onOpenCard }: ViewProps = $props();

  const locale = $derived(board.workspace.locale);
  const regional = $derived(board.workspace.regional);
  const templates = $derived(board.board?.cardProperties ?? []);
  const template = $derived(displayProperty(view.fields.dateProperty, templates));
  const clock = new MinuteClock();
  /** Today, on the reader's calendar. */
  const today = $derived(dateOfInstant(clock.now, regional.timeZone));

  const period = new Period(
    untrack(() => view.id),
    untrack(() => today),
  );
  const month = $derived(period.month);
  const selected = $derived(period.selected);
  const scale = $derived(period.scale);
  /** The box is wide enough for bars. */
  const box = new PaneRoom();
  const bars = $derived(box.columns);
  const sheet = $derived(overlayPresentation(page.stage) === 'sheet');
  /**
   * The cards of the picked day are open over the grid of bars. Closed
   * whenever the box goes from bars to the small grid or back, so that a box
   * that widens again does not bring them back unasked.
   */
  let dayOpen = $derived<boolean>(bars && false);
  /** The cards without a date: a column at the right, or a sheet on a compact page. */
  const undated = new UndatedColumn({
    sheet: () => sheet,
    onOpenCard: (cardId: string) => {
      onOpenCard(cardId);
    },
    onopen: () => {
      dayOpen = false;
    },
  });

  /**
   * The first day of the week drawn, in a week. A string, so that the arrow
   * keys inside a week change nothing below it: the week, and every card's
   * place in it, are worked out again only for another week.
   */
  const weekFirst = $derived(
    scale === 'week' ? weekOf(selected, regional.weekStart)[0] : undefined,
  );
  const weeks = $derived(
    weekFirst ? [weekOf(weekFirst, regional.weekStart)] : weeksOfMonth(month, regional.weekStart),
  );
  const placed = $derived(new Placed(cards, template, regional, locale, weeks.flat()));

  const finger = $derived(fingerPointer.current);

  /** The box, for finding the button of a day in it. Not reactive: read on a key press. */
  let body: HTMLElement | undefined;

  /**
   * The box: kept for the focus, and measured. The column of cards without a
   * date lies over it: the focus going into the box (Shift+Tab out of the
   * column, an arrow key) means the reader has left the column, which closes
   * so that nothing focused is under it. For `{@attach}`.
   */
  function attachBody(element: HTMLElement): () => void {
    body = element;
    const left = (): void => {
      undated.left();
    };
    element.addEventListener('focusin', left);
    return () => {
      element.removeEventListener('focusin', left);
    };
  }

  /** Puts the focus on the button of the picked day. */
  function focusDay(): void {
    body
      ?.querySelector<HTMLElement>(
        `[data-day="${selected}"][data-day-button], [data-day="${selected}"] [data-day-button]`,
      )
      ?.focus();
  }

  /** Picks a day; a day of another month brings that month into view. */
  function select(date: CalendarDate): void {
    period.select(date);
  }

  /** A month or a week on or back. */
  function step(by: number): void {
    dayOpen = false;
    period.step(by, today);
  }

  function goToday(): void {
    dayOpen = false;
    period.goTo(today);
  }

  function pick(date: CalendarDate): void {
    undated.open = false;
    select(date);
    if (bars) dayOpen = true;
  }

  /** An arrow key: the picked day moves, and the focus with it. */
  async function move(by: number): Promise<void> {
    select(addDays(selected, by));
    await tick();
    focusDay();
  }

  const writable = $derived(datesWritable(template));
  const rows = $derived({
    placed,
    templates,
    shownProperties: view.fields.shownProperties,
    users: board.workspace.users,
    cardHref,
    board,
    onChangeDate: writable
      ? (cardId: string, line: HTMLElement | undefined) => void changeDate(cardId, line)
      : undefined,
  });
  const fixed = $derived(
    writable
      ? undefined
      : m.calendar_dates_fixed({
          property: template?.name ?? m['PropertyType.CreatedTime'](),
        }),
  );
  const lists = $derived({ ...rows, today, fixed });

  // ------------------------------------------------------------ dragging

  const drag = new CardDrag({
    days: (cardId: string) => placed.span(cardId),
    enabled: () => writable && board.canEdit,
    ondrop: (cardId: string, grip: BarGrip, days: Days) => void dropCard(cardId, grip, days),
  });

  /** A card let go on other days: its date is written, and the board shows it at once. */
  async function dropCard(cardId: string, grip: BarGrip, days: Days): Promise<void> {
    const card = placed.card(cardId);
    if (!card || !template || !datesWritable(template)) return;
    writing += 1;
    void writeDays(board, card, template, grip, days, regional.timeZone).finally(() => {
      writing -= 1;
    });
    // A card given a date leaves "No date", and the focus on its line with it.
    await tick();
    if (focusAdrift(undated.button)) undated.button?.focus();
  }

  /**
   * How many drops are being written: until the backend has answered them,
   * the calendar says it is settling (`data-settling`), as a board does.
   */
  let writing = $state(0);

  /** What the chip that follows a dragged card says: its title and the days it would go to. */
  const chip = $derived.by(() => {
    const preview = drag.preview;
    const card = preview ? placed.card(preview.id) : undefined;
    if (!preview || !card) return undefined;
    const days = preview.days;
    const when = days
      ? days.first === days.last
        ? displayDay(days.first, locale)
        : m.gantt_days_range({
            first: displayDay(days.first, locale),
            last: displayDay(days.last, locale),
          })
      : undefined;
    return { title: placed.title(card), when };
  });

  // -------------------------------------------------------- making a card

  /** Cards can be made on a day: by someone who may edit, where the day is a date to set. */
  const creatable = $derived(writable && board.canEdit);

  const maker = new CardMaker(() => ({ board, view, templates, template }), {
    select,
    focusDay,
    onOpenCard: (cardId: string) => {
      onOpenCard(cardId);
    },
  });
  const createOn = (date: CalendarDate): void => {
    void maker.on(date);
  };

  // The header's "New" makes the card on the day picked.
  $effect(() => {
    if (!creatable) return;
    return registerNewCard(view.id, () => maker.on(period.selected));
  });

  // ------------------------------------------------------ changing a date

  /** The card whose date the picker of a day changes, and whether the picker is open. */
  let changing = $state<string | undefined>(undefined);
  let pickerOpen = $state(false);
  let changingLine = $state<HTMLElement | null>(null);

  /** "Change date" in a card's menu: the lists close, and the picker opens by the picked day. */
  async function changeDate(cardId: string, line?: HTMLElement): Promise<void> {
    // A line of a list in the calendar's box stays on screen: the picker hangs by it.
    changingLine = line && body?.contains(line) ? line : null;
    dayOpen = false;
    undated.open = false;
    changing = cardId;
    await tick();
    pickerOpen = true;
  }

  /**
   * The board kept a card's move: the calendar goes to its day, and the
   * focus with it if it is on the day the card was on.
   */
  async function moved(date: CalendarDate): Promise<void> {
    const onDay = body?.contains(document.activeElement) ?? false;
    select(date);
    await tick();
    if (onDay) focusDay();
  }

  /** The picker has closed: the focus goes to the card's day, or back to "No date". */
  async function pickerClosed(dated: boolean): Promise<void> {
    await tick();
    // Back to the "…" of the line it came from, where that line is still there.
    const back = changingLine?.isConnected
      ? changingLine.querySelector<HTMLElement>('[data-card-menu]')
      : null;
    changingLine = null;
    if (back) back.focus();
    else if (dated) focusDay();
    else undated.button?.focus();
  }
</script>

<div
  data-testid="calendar"
  data-settling={writing > 0 ? '' : undefined}
  data-month="{month.year}-{String(month.month).padStart(2, '0')}"
  data-selected={selected}
  data-scale={scale}
  data-layout={box.measured ? (bars ? 'bars' : 'mini') : undefined}
  class="flex h-full min-h-0 flex-col"
>
  <Toolbar
    title={periodTitle(scale, weeks, month, locale, today)}
    {scale}
    onscale={(next: Scale) => {
      dayOpen = false;
      period.scale = next;
    }}
    onstep={step}
    ontoday={goToday}
    undated={placed.undated.length}
    undatedOpen={undated.open}
    bind:undatedButton={undated.button}
    onundated={() => void undated.toggle()}
  />

  <!-- Measured here, outside the scrollbar a low window gives the box: the width does not depend on it. -->
  <div class="relative flex min-h-0 flex-1 flex-col" {@attach box.attach} {@attach drag.attach}>
    <!-- A low window scrolls the small grid and its list together; the list keeps room for two cards. -->
    <div
      class={['relative flex min-h-0 flex-1 flex-col', !bars && 'overflow-y-auto']}
      {@attach attachBody}
    >
      {#if !box.measured}
        <!-- Measured first: what is drawn depends on the width. -->
      {:else if bars}
        <MonthGrid
          {weeks}
          week={scale === 'week'}
          shownOf={(card: Card) => faceProperties(card, templates, view.fields.shownProperties)}
          {placed}
          {month}
          {today}
          {selected}
          {finger}
          {cardHref}
          {onOpenCard}
          onpick={pick}
          onmove={(by: number) => void move(by)}
          oncreate={creatable ? createOn : undefined}
          draggable={writable && board.canEdit}
          dragging={drag.preview}
        />
      {:else if scale === 'week'}
        <div
          class={[
            'flex min-h-0 flex-1 flex-col',
            board.canEdit && 'pb-[calc(var(--safe-bottom)+5rem)]',
          ]}
        >
          <WeekAgenda
            week={weeks[0] ?? []}
            {placed}
            {today}
            {selected}
            rows={{ ...rows, onOpenCard }}
            {fixed}
            onpick={pick}
            onmove={(by: number) => void move(by)}
            oncreate={creatable ? createOn : undefined}
          />
        </div>
      {:else}
        <MiniMonth
          {weeks}
          {placed}
          {month}
          {today}
          {selected}
          onpick={pick}
          onmove={(by: number) => void move(by)}
        />
        <!-- Room at the bottom for the round "New" of a folded header, which would cover the last line. -->
        <div
          class={[
            'flex min-h-36 flex-1 flex-col border-t',
            board.canEdit ? 'pb-[calc(var(--safe-bottom)+5rem)]' : 'pb-(--safe-bottom)',
          ]}
        >
          <DayList
            date={selected}
            {...lists}
            {onOpenCard}
            oncreate={creatable ? createOn : undefined}
          />
        </div>
      {/if}
    </div>

    {#if undated.open && !sheet}
      <!--
        Over the grid, at its right: the proposal's column stands beside the
        grid only from a pane of 54rem, a width this step does not add.
      -->
      <div
        data-testid="undated-panel"
        class={[
          'absolute inset-y-0 right-0 z-30 flex w-80 max-w-full flex-col border-l bg-background pb-(--safe-bottom) shadow-lg',
          // While a card is dragged the days under the column can be reached.
          drag.preview && 'pointer-events-none opacity-30',
        ]}
        {@attach undated.attach}
      >
        <Undated
          {...rows}
          onOpenCard={(cardId: string) => void undated.openCard(cardId)}
          onclose={undated.close}
        />
      </div>
    {/if}
  </div>
</div>

<DayPicker
  bind:cardId={changing}
  bind:open={pickerOpen}
  line={changingLine}
  {board}
  {placed}
  {template}
  {selected}
  {today}
  {sheet}
  onmoved={(date: CalendarDate) => void moved(date)}
  onclosed={(dated: boolean) => void pickerClosed(dated)}
/>

{#if sheet}
  <BottomSheet
    bind:open={undated.open}
    returnFocus={() => undated.button}
    title={m.calendar_undated_count({ count: placed.undated.length })}
  >
    <Undated {...rows} onOpenCard={(cardId: string) => void undated.openCard(cardId)} />
  </BottomSheet>
{/if}

{#if chip && drag.at}
  <div
    data-testid="calendar-drag-chip"
    aria-hidden="true"
    class="pointer-events-none fixed z-50 flex max-w-60 flex-col rounded-sm bg-popover px-2 py-1 text-xs text-popover-foreground shadow-lg ring-1 ring-border"
    style:left="{drag.at.x + 12}px"
    style:top="{drag.at.y + 12}px"
  >
    <span class="truncate font-medium">{chip.title}</span>
    {#if chip.when}<span class="truncate text-muted-foreground">{chip.when}</span>{/if}
  </div>
{/if}

{#if bars}
  <DayOverlay
    bind:open={dayOpen}
    {sheet}
    {finger}
    {selected}
    {locale}
    {lists}
    mayFocusDay={() => !undated.open}
    {focusDay}
    {onOpenCard}
    oncreate={creatable ? createOn : undefined}
  />
{/if}
