<!--
  The Gantt view: cards as bars on the days of one date property, grouped like a
  table, with interactive four-kind dependency arrows (`FS`/`SS`/`FF`/`SF` and
  lag days), drag-to-reschedule, keyboard navigation, and undated card list.
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';

  import BottomSheet from '$lib/components/sheet/BottomSheet.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { overlayPresentation } from '$lib/layout/presentation.ts';
  import { page, Room } from '$lib/layout/room.svelte.ts';
  import { datesWritable } from '$lib/logic/calendar.ts';
  import { dateOfInstant, type CalendarDate } from '$lib/logic/dates.ts';
  import { barBox, DAY_WIDTH, xOfDay, zoomOf, type Axis } from '$lib/logic/gantt.ts';
  import type { Days } from '$lib/logic/dependencies.ts';
  import type { GanttZoom } from '$lib/logic/view-members.ts';
  import { registerNewCard } from '$lib/state/index.ts';

  import { MinuteClock } from '../calendar/clock.svelte.ts';
  import CardMenu from '../calendar/CardMenu.svelte';
  import DayPicker from '../calendar/DayPicker.svelte';
  import Undated from '../calendar/Undated.svelte';
  import { writeDays } from '../calendar/write.ts';
  import { UndatedColumn } from '../calendar/undated-column.svelte.ts';
  import type { ViewProps } from '../props.ts';

  import ArrowControls from './ArrowControls.svelte';
  import { ArrowEdit } from './arrow-edit.svelte.ts';
  import Arrows from './Arrows.svelte';
  import AxisHeader from './AxisHeader.svelte';
  import { BarDrag, type BarGrip } from './bar-drag.svelte.ts';
  import { BarFocus } from './bar-focus.svelte.ts';
  import BarRow from './BarRow.svelte';
  import { GanttChart } from './chart.svelte.ts';
  import { DaysHold } from './days-hold.svelte.ts';
  import GroupRow from './GroupRow.svelte';
  import { ganttCardMaker } from './making.ts';
  import { daysLabel } from './names.ts';
  import RelationEditor from './RelationEditor.svelte';
  import Toolbar from './Toolbar.svelte';

  let { board, view, cards, cardHref, onOpenCard }: ViewProps = $props();

  const locale = $derived(board.workspace.locale);
  const clock = new MinuteClock();
  /** Today, on the reader's calendar. */
  const today = $derived(dateOfInstant(clock.now, board.workspace.regional.timeZone));
  /** The zoom a reader who may not change the view chose: theirs until another view is shown. */
  let ownZoom = $state<GanttZoom | undefined>(undefined);
  const zoom = $derived((board.canEdit ? undefined : ownZoom) ?? zoomOf(view.fields.ganttZoom));

  /** The box the chart scrolls in, measured: the axis is at least as long as it is wide. */
  const box = new Room();

  /** The size of 1rem in pixels, at the reader's text size. */
  function rem(): number {
    return parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
  }

  const minDays = $derived(Math.ceil(box.width / (DAY_WIDTH[zoom] * rem()) / 28) * 28);

  const drag: BarDrag = new BarDrag({
    axis: (): Axis => chart.axis,
    days: (cardId: string) => chart.placed.span(cardId),
    enabled: () => writable,
    scroller: () => scroller,
    titles,
    room: daysRoom,
    rem,
    ondrop: dropBar,
  });

  const chart: GanttChart = new GanttChart({
    board: () => board,
    view: () => view,
    cards: () => cards,
    today: () => today,
    zoom: () => zoom,
    minDays: () => minDays,
    reach: (): Days | undefined => drag.reach,
  });

  /** Bars can be dragged: by someone who may edit, where they are placed by a date to set. */
  const writable = $derived(board.canEdit && datesWritable(chart.template));

  /** A bar let go on other days: its card's date is written, and the board shows it at once. */
  function dropBar(cardId: string, grip: BarGrip, days: Days): void {
    const card = chart.placed.card(cardId);
    const template = chart.template;
    if (!card || !template || !datesWritable(template)) return;
    writing += 1;
    void writeDays(board, card, template, grip, days, board.workspace.regional.timeZone).finally(
      () => {
        writing -= 1;
      },
    );
  }

  /** Drops being written: the chart says it is settling until they are answered. */
  let writing = $state(0);

  /** The bar being dragged as it is drawn meanwhile. */
  const dragged = $derived.by(() => {
    const preview = drag.preview;
    if (!preview) return undefined;
    const days = daysLabel(preview.days, locale, Number(today.slice(0, 4)), (first, last) =>
      m.gantt_days_range({ first, last }),
    );
    return {
      id: preview.id,
      first: preview.days.first,
      box: barBox(axis, preview.days),
      days,
      label: () => drag.label,
    };
  });

  const axis = $derived(chart.axis);
  /** The middle of today, in rem from the start of the axis. */
  const todayX = $derived(xOfDay(axis, today) + axis.dayWidth / 2);

  /** The box that scrolls. Reactive (raw) so arrow clipping attaches once mounted. */
  let scroller = $state.raw<HTMLElement | undefined>(undefined);

  /** How wide the column of titles is, in pixels. */
  function titles(): number {
    return scroller?.querySelector<HTMLElement>('[data-gantt-corner]')?.offsetWidth ?? 0;
  }

  /** How wide the days in sight are, in pixels: the box less the column of titles. */
  function daysRoom(): number {
    return scroller ? scroller.clientWidth - titles() : 0;
  }

  /** Scrolls the days so that the middle of `date` is `part` of the way into what is in sight. */
  function showDay(date: CalendarDate, part = 1 / 3): void {
    if (!scroller) return;
    const x = (xOfDay(axis, date) + axis.dayWidth / 2) * rem();
    scroller.scrollLeft = Math.max(0, x - daysRoom() * part);
    hold.release();
  }

  /** Another zoom: the view's where the reader may change the view, theirs for the visit where not. */
  function setZoom(next: GanttZoom): void {
    if (next === zoom) return;
    if (board.canEdit) void board.setViewFields(view.id, { ganttZoom: next }, 'ganttZoom');
    else ownZoom = next;
  }

  /** The days in sight stay in sight when the zoom or the start of the axis changes. */
  const hold = new DaysHold({
    axis: () => axis,
    today: () => today,
    scroller: () => scroller,
    room: daysRoom,
    rem,
  });

  /** Starts with today in sight, closes the undated column on focusin, and wires bar drag. */
  function attachScroller(element: HTMLElement): () => void {
    scroller = element;
    untrack(() => {
      showDay(today);
    });
    const left = (): void => {
      undated.left();
    };
    element.addEventListener('focusin', left);
    const detach = drag.attach(element);
    return () => {
      detach();
      element.removeEventListener('focusin', left);
      scroller = undefined;
    };
  }

  // ------------------------------------------------------------- focus

  const focus = new BarFocus({
    chart: () => chart,
    scroller: () => scroller,
    undatedButton: () => undated.button,
    onOpenCard: (cardId: string) => {
      onOpenCard(cardId);
    },
    rem,
  });

  /** Focuses the next bar on a loop, or opens the undated list focused on its first loop card. */
  async function showLoop(): Promise<void> {
    if (chart.loopCount > 0) {
      focus.focusNext('loop');
      return;
    }
    if (!undated.open) await undated.toggle();
    await tick();
    document
      .querySelector<HTMLElement>('[data-testid="undated-list"] [data-mark="loop"]')
      ?.closest('[data-card]')
      ?.querySelector<HTMLElement>('a')
      ?.focus();
  }

  // ------------------------------------------------------- making a card

  const makeCard = ganttCardMaker({
    board: () => board,
    view: () => view,
    template: () => chart.template,
    axis: () => axis,
    today: () => today,
    scroller: () => scroller,
    room: daysRoom,
    rem,
    onOpenCard: focus.openCard,
  });

  $effect(() => {
    if (!writable) return;
    return registerNewCard(view.id, makeCard);
  });

  // ------------------------------------------------------ changing a date

  /** The card whose date the picker of days changes, the picker, and the row it hangs by. */
  let changing = $state<string | undefined>(undefined);
  let pickerOpen = $state(false);
  let changingRow = $state<HTMLElement | null>(null);
  /** The card last named to the picker: the picker forgets it as it closes. Not reactive. */
  let changed: string | undefined;

  /** "Change date" in a card's "…": the picker opens by the card's row. */
  async function changeDate(cardId: string): Promise<void> {
    changingRow =
      scroller?.querySelector<HTMLElement>(`[data-row="${CSS.escape(cardId)}"]`) ?? null;
    undated.open = false;
    changing = cardId;
    changed = cardId;
    await tick();
    pickerOpen = true;
  }

  /** The picker has closed: the focus goes back to the card's bar, or to "No date". */
  async function pickerClosed(): Promise<void> {
    await tick();
    const cardId = changed;
    const row = cardId
      ? scroller?.querySelector<HTMLElement>(`[data-row="${CSS.escape(cardId)}"]`)
      : null;
    changingRow = null;
    const back = row?.querySelector<HTMLElement>('[data-bar]');
    if (back) back.focus();
    else undated.button?.focus();
  }

  /** The board kept a move to `date`: the day comes into sight, if it is not. */
  function moved(date: CalendarDate): void {
    if (!scroller) return;
    const x = (xOfDay(axis, date) + axis.dayWidth / 2) * rem() - scroller.scrollLeft;
    if (x < 0 || x > daysRoom()) showDay(date);
  }

  // ------------------------------------------------ editing a relation

  const relationsEditable = $derived(board.canEdit && chart.dependency !== undefined);
  const arrowEdit = new ArrowEdit({
    editable: () => relationsEditable,
    arrows: () => chart.arrows,
    hasBar: (cardId) => chart.bars.has(cardId),
    fallbackBar: () => focus.stop,
    scroller: () => scroller,
    rem,
    closeUndated: () => {
      undated.open = false;
    },
    focusBar: (cardId) => {
      focus.focusBar(cardId);
    },
  });
  const toggleParent = (cardId: string): void => {
    arrowEdit.close(cardId);
    chart.toggleParent(cardId);
  };

  // ------------------------------------------------ cards without a date

  const sheet = $derived(overlayPresentation(page.stage) === 'sheet');
  const undated = new UndatedColumn({
    sheet: () => sheet,
    onOpenCard: (cardId: string) => {
      onOpenCard(cardId);
    },
  });

  const undatedRows = $derived({
    placed: chart.placed,
    templates: board.board?.cardProperties ?? [],
    shownProperties: view.fields.shownProperties,
    users: board.workspace.users,
    board,
    cardHref,
    hint: (property: string) => m.gantt_undated_hint({ property }),
    loops: chart.undatedLoops,
  });
</script>

<div
  data-testid="gantt"
  data-settling={writing > 0 ? '' : undefined}
  data-zoom={zoom}
  data-first={axis.first}
  data-last={axis.last}
  data-edges={chart.relations.edges.length}
  data-late={chart.relations.late.size}
  class="flex h-full min-h-0 flex-col [--gantt-row:2.25rem] [--gantt-title:7rem] @columns/pane:[--gantt-title:14rem] roomy:[--gantt-row:2.75rem]"
  style:--gantt-width="{axis.width}rem"
>
  <Toolbar
    ontoday={() => {
      showDay(today);
    }}
    related={chart.dependency !== undefined}
    late={chart.relations.late.size}
    onlate={() => {
      focus.focusNext('late');
    }}
    loops={chart.loopCount > 0 ? chart.loopCount : chart.undatedLoops.size}
    loopsUndated={chart.loopCount === 0}
    onloop={() => void showLoop()}
    {zoom}
    onzoom={setZoom}
    undated={chart.placed.undated.length}
    undatedLoops={chart.loopCount > 0 ? chart.undatedLoops.size : 0}
    undatedCard={focus.openedUndated}
    undatedOpen={undated.open}
    bind:undatedButton={undated.button}
    onundated={() => void undated.toggle()}
  />

  <!-- Measured here, outside the scrollbars of the box inside: the width does not depend on them. -->
  <div class="relative flex min-h-0 flex-1 flex-col" {@attach box.attach}>
    {#if box.measured}
      <div
        data-testid="gantt-scroller"
        tabindex="-1"
        class="min-h-0 flex-1 scroll-pt-12 overflow-auto overscroll-contain outline-none"
        {@attach attachScroller}
      >
        <div
          class="relative flex min-h-full w-[calc(var(--gantt-title)+var(--gantt-width))] min-w-full flex-col"
        >
          <AxisHeader {axis} scale={chart.scale} {todayX} {today} {locale} />
          <div class="relative flex-1 pb-[calc(var(--fab-room,0px)+var(--safe-bottom))]">
            <!-- Under the rows: a line at each week or month, and the line of today. -->
            <div
              aria-hidden="true"
              class="pointer-events-none absolute inset-y-0 left-(--gantt-title) w-(--gantt-width)"
            >
              {#each chart.scale.lines as x (x)}
                <div class="absolute inset-y-0 w-px bg-border/60" style:left="{x}rem"></div>
              {/each}
              <div
                data-testid="gantt-today-line"
                data-day={today}
                class="absolute inset-y-0 w-0.5 -translate-x-1/2 bg-primary"
                style:left="{todayX}rem"
              ></div>
            </div>
            <Arrows
              arrows={chart.arrows}
              width={axis.width}
              rows={chart.rows.length}
              activeKey={arrowEdit.activeKey}
            />
            <div aria-hidden="true" class="pointer-events-none absolute inset-0">
              <div class="sticky left-0 h-full w-(--gantt-title) border-r bg-background"></div>
            </div>
            <ul role="list" aria-label={m.gantt_rows()} class="relative">
              {#each chart.blocks as block, at (at)}
                <div
                  role="presentation"
                  class="[contain-intrinsic-block-size:calc(var(--gantt-row)*var(--rows))] [content-visibility:auto]"
                  style:--rows={block.length}
                >
                  {#each block as line (`${line.kind}:${line.id}`)}
                    {#if line.kind === 'group'}
                      <GroupRow {line} />
                    {:else}
                      <BarRow
                        {line}
                        current={line.id === focus.stop}
                        href={cardHref(line.id)}
                        viaTitle={line.id === focus.viaTitle}
                        pointed={line.id === focus.pointed}
                        onopen={focus.openCard}
                        onopentitle={focus.openFromTitle}
                        onkeys={focus.barKeys}
                        onfocused={focus.barFocused}
                        ontoggle={toggleParent}
                        draggable={writable}
                        menu={board.canEdit ? rowMenu : undefined}
                        dragged={dragged?.id === line.id ? dragged : undefined}
                      />
                    {/if}
                  {/each}
                </div>
              {:else}
                <li
                  data-testid="gantt-empty"
                  class="sticky left-0 px-3 py-6 text-center text-muted-foreground"
                  style:width="{box.width}px"
                >
                  {m.gantt_empty()}
                </li>
              {/each}
            </ul>
            <ArrowControls
              arrows={chart.arrows}
              width={axis.width}
              rows={chart.rows.length}
              editable={relationsEditable}
              {scroller}
              selectedKey={arrowEdit.open ? arrowEdit.editing?.key : undefined}
              hoveredKey={arrowEdit.hovered}
              focusedKey={arrowEdit.focused}
              onhover={arrowEdit.setHovered}
              onfocuskey={arrowEdit.setFocused}
              onselect={arrowEdit.select}
              onfocused={arrowEdit.focusedButton}
            />
          </div>
        </div>
      </div>
    {/if}

    {#if undated.open && !sheet}
      <div
        data-testid="undated-panel"
        class="absolute inset-y-0 right-0 z-40 flex w-80 max-w-full flex-col border-l bg-background pb-(--safe-bottom) shadow-lg"
        {@attach undated.attach}
      >
        <Undated
          {...undatedRows}
          onOpenCard={(cardId: string) => void undated.openCard(cardId)}
          onclose={undated.close}
        />
      </div>
    {/if}
  </div>
</div>

{#snippet rowMenu(cardId: string)}
  {@const card = chart.placed.card(cardId)}
  {#if card}
    <CardMenu
      {board}
      {card}
      title={chart.placed.title(card)}
      dated
      quiet
      onChangeDate={writable ? (id: string) => void changeDate(id) : undefined}
    />
  {/if}
{/snippet}

<DayPicker
  bind:cardId={changing}
  bind:open={pickerOpen}
  line={changingRow}
  {board}
  placed={chart.placed}
  template={chart.template}
  selected={changing ? (chart.placed.span(changing)?.first ?? today) : today}
  {today}
  {sheet}
  onmoved={moved}
  onclosed={() => void pickerClosed()}
/>

<RelationEditor
  bind:arrow={arrowEdit.editing}
  bind:open={arrowEdit.open}
  anchor={arrowEdit.anchor}
  {board}
  dependencyPropertyId={chart.dependency?.id}
  {sheet}
  onclosed={arrowEdit.closed}
/>

{#if sheet}
  <BottomSheet
    bind:open={undated.open}
    returnFocus={() => undated.button}
    title={m.calendar_undated_count({ count: chart.placed.undated.length })}
  >
    <Undated {...undatedRows} onOpenCard={(cardId: string) => void undated.openCard(cardId)} />
  </BottomSheet>
{/if}
