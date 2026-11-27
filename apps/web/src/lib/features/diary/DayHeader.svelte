<!--
  The head of a diary's page: the diary's name and the days written in a row,
  the day of the year shown (`November 21`), the way to the day before and
  the day after, back to today, and to any day picked from the diary's
  calendar of written days (`WrittenDays`; its year shown too, below this
  year's), and the diary's menu (renaming it, its deleted entries to bring
  back, deleting it).

  "Today" is the reader's: it says it is the day shown (`aria-current`) and
  takes them back there from any other, also after midnight came while the
  page was open.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import CalendarIcon from '@lucide/svelte/icons/calendar-days';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import FlameIcon from '@lucide/svelte/icons/flame';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { insets } from '$lib/layout/room.svelte.ts';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import { dateIn, monthDayOf, yearOf, type MonthDay } from '$lib/logic/diary-days.ts';
  import { streakOf } from '$lib/logic/diary-streak.ts';
  import { displayDay } from '$lib/logic/format.ts';
  import type { DiaryCalendar } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Popover from '$lib/ui/popover/index.js';

  import WrittenDays from './WrittenDays.svelte';

  interface Props {
    /** The diary's name as it is shown. */
    title: string;
    calendar: DiaryCalendar;
    monthDay: MonthDay;
    today: CalendarDate;
    locale: string;
    /** The first day of the week, 0 for Sunday. */
    weekStart: number;
    onStep: (by: 1 | -1) => void;
    onToday: () => void;
    /** A day was picked, of any year up to today. */
    onPick: (date: CalendarDate) => void;
    onRename: () => void;
    /** Opens the diary's trash. */
    onTrash: () => void;
    onDelete: () => void;
  }

  let {
    title,
    calendar,
    monthDay,
    today,
    locale,
    weekStart,
    onStep,
    onToday,
    onPick,
    onRename,
    onTrash,
    onDelete,
  }: Props = $props();

  /** The day of the year shown, written for this leap year: `02-29` is one of them. */
  const shown = $derived(`2000-${monthDay}` as CalendarDate);
  const isToday = $derived(monthDayOf(today) === monthDay);
  let picking = $state(false);
  /**
   * The calendar opens on the day shown in its latest year up to today (last
   * year's while this year's is still to come), or on today where neither
   * year has it (`02-29`): a month whose days can be picked.
   */
  const opening = $derived(
    [yearOf(today), yearOf(today) - 1]
      .map((year) => dateIn(year, monthDay))
      .find((date) => date !== undefined && date <= today) ?? today,
  );
  const streak = $derived(streakOf(calendar.runs, today));

  let picker = $state<HTMLElement | null>(null);

  /** The calendar opens with its day in hand: the one it opens on, else the first of the month. */
  function focusDay(event: Event): void {
    event.preventDefault();
    picker?.querySelector<HTMLElement>('[data-bits-day][tabindex="0"]')?.focus();
    void tick().then(() => {
      if (!picking || !picker) return;
      const target = picker.querySelector<HTMLElement>('[data-bits-day][tabindex="0"]');
      if (target && document.activeElement !== target) target.focus();
    });
  }

  function pick(date: CalendarDate): void {
    picking = false;
    onPick(date);
  }

  const actions = $derived([
    { id: 'rename', label: m.diary_rename(), icon: PencilIcon, run: onRename },
    { id: 'trash', label: m.diary_restore_deleted(), icon: RotateCcwIcon, run: onTrash },
    {
      id: 'delete',
      label: m.shell_delete_diary(),
      icon: Trash2Icon,
      destructive: true,
      separated: true,
      run: onDelete,
    },
  ]);
</script>

<header class="flex flex-col gap-1 border-b pb-3" data-testid="diary-head">
  <div class="flex items-center gap-2">
    <p class="min-w-0 flex-1 truncate text-sm text-muted-foreground" data-testid="diary-title">
      {title}
    </p>
    {#if calendar.counted && streak.current > 0}
      <p
        class="flex shrink-0 items-center gap-1 text-sm text-muted-foreground"
        data-testid="diary-streak-badge"
      >
        <FlameIcon class="size-4" />
        {m.diary_streak({ count: streak.current })}
      </p>
    {/if}
    <ActionMenu title={m.diary_menu({ title })} {actions} align="end">
      {#snippet trigger(props: Record<string, unknown>)}
        <IconButton {...props} label={m.diary_menu({ title })} data-testid="diary-menu">
          <EllipsisIcon />
        </IconButton>
      {/snippet}
    </ActionMenu>
  </div>
  <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
    <h1 class="min-w-0 flex-1 text-2xl font-semibold tracking-tight" data-testid="diary-day">
      {displayDay(shown, locale)}
    </h1>
    <div class="flex items-center gap-1">
      <IconButton
        label={m.diary_previous_day()}
        data-testid="diary-previous"
        onclick={() => {
          onStep(-1);
        }}
      >
        <ChevronLeftIcon />
      </IconButton>
      <Button
        variant="outline"
        size="sm"
        class="roomy:min-h-touch"
        aria-current={isToday ? 'date' : undefined}
        data-testid="diary-today"
        onclick={onToday}
      >
        {m.diary_today()}
      </Button>
      <IconButton
        label={m.diary_next_day()}
        data-testid="diary-next"
        onclick={() => {
          onStep(1);
        }}
      >
        <ChevronRightIcon />
      </IconButton>
      <Popover.Root
        bind:open={picking}
        onOpenChange={(open: boolean) => {
          if (open) void calendar.showMonth(opening);
        }}
      >
        <Popover.Trigger>
          {#snippet child({ props }: { props: Record<string, unknown> })}
            <IconButton {...props} label={m.diary_pick_day()} data-testid="diary-pick">
              <CalendarIcon />
            </IconButton>
          {/snippet}
        </Popover.Trigger>
        <Popover.Content
          align="end"
          collisionPadding={insets.collisionPadding}
          aria-label={m.diary_pick_day()}
          class="w-auto p-1"
          data-testid="diary-picker"
          bind:ref={picker}
          onOpenAutoFocus={focusDay}
        >
          <WrittenDays {calendar} {streak} {opening} {today} {locale} {weekStart} onPick={pick} />
        </Popover.Content>
      </Popover.Root>
    </div>
  </div>
</header>
