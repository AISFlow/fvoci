<!--
  Modal dialog for configuring a course's shared study schedule (editors/admins)
  and manually redistributing either the learner's own incomplete units (every
  enrolled member, including viewers and guests) or the course's shared dates
  (editors/admins, with confirmation).
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import CalendarClockIcon from '@lucide/svelte/icons/calendar-clock';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import XIcon from '@lucide/svelte/icons/x';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import { addDays, parseCalendarDate } from '$lib/logic/dates.ts';
  import { validateScheduleWindow } from '$lib/logic/course-schedule.ts';
  import { STUDY_DAYS_WEEKDAYS, STUDY_WEEKDAYS, type CourseStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Checkbox } from '$lib/ui/checkbox/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import {
    fieldErrorsFromBackend,
    issueMessage,
    weekdayLabel,
    type ScheduleFieldErrors,
  } from './course-schedule-labels.ts';

  interface Props {
    open: boolean;
    course: CourseStore;
  }

  let { open = $bindable(), course }: Props = $props();

  const uid = $props.id();
  const compact = $derived(page.stage === 'compact');
  const viewReady = $derived(course.schedule.view !== null);
  const saved = $derived(course.schedule.schedule);
  const today = $derived(course.schedule.today);
  const todayCount = $derived(course.schedule.todayCount);
  const overdueCount = $derived(course.schedule.overdueCount);
  const busy = $derived(course.schedule.busy || !viewReady);

  function defaultTarget(base: string): string {
    const parsed = parseCalendarDate(base);
    return parsed ? addDays(parsed, 14) : base;
  }

  let startDate = $state('');
  let targetDate = $state('');
  let studyDays = $state(STUDY_DAYS_WEEKDAYS);
  let autoAssign = $state(true);
  let fromDate = $state('');
  let errors = $state.raw<ScheduleFieldErrors>({});
  let confirmSharedOpen = $state(false);
  let snapshotKey = '';

  // Snapshot store values on open, course switch, or initial view arrival so background refreshes never wipe edits.
  $effect(() => {
    const key = open ? `${course.courseId}:${viewReady}` : '';
    if (key === snapshotKey) return;
    snapshotKey = key;
    if (!open) {
      confirmSharedOpen = false;
      return;
    }
    untrack(() => {
      const current = course.schedule.schedule;
      const baseToday = course.schedule.today;
      startDate = current?.startDate ?? baseToday;
      targetDate = current?.targetDate ?? defaultTarget(baseToday);
      studyDays = current?.studyDays ?? STUDY_DAYS_WEEKDAYS;
      autoAssign = current === null;
      fromDate = baseToday;
      errors = {};
    });
  });

  closeOnBack(
    () => open && !confirmSharedOpen,
    () => {
      open = false;
    },
  );

  function toggleDay(bit: number): void {
    studyDays = (studyDays & bit) !== 0 ? studyDays & ~bit : studyDays | bit;
    if (errors.studyDays) errors = { ...errors, studyDays: undefined };
  }

  function validateWindow(
    startText: string,
    targetText: string,
    days: number,
    startField: 'startDate' | 'fromDate',
  ): boolean {
    const issues = validateScheduleWindow(startText, targetText, days, startField);
    if (issues.length === 0) {
      errors = {};
      return true;
    }
    const next: ScheduleFieldErrors = {};
    for (const issue of issues) {
      if (issue.field === 'startDate' && !next.startDate) next.startDate = issueMessage(issue);
      else if (issue.field === 'fromDate' && !next.fromDate) next.fromDate = issueMessage(issue);
      else if (issue.field === 'targetDate' && !next.targetDate) {
        next.targetDate = issueMessage(issue);
      } else if (issue.field === 'studyDays' && !next.studyDays) {
        next.studyDays = issueMessage(issue);
      }
    }
    errors = next;
    return false;
  }

  async function saveSchedule(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!course.canEdit || busy) return;
    if (!validateWindow(startDate, targetDate, studyDays, 'startDate')) return;
    const result = await course.schedule.saveSchedule({
      startDate,
      targetDate,
      studyDays,
      autoAssign,
    });
    if (result.ok) {
      errors = {};
    } else if (result.error) {
      errors = fieldErrorsFromBackend(result.error);
    }
  }

  async function clearSchedule(): Promise<void> {
    if (!course.canEdit || busy || !saved) return;
    const ok = await course.schedule.deleteSchedule();
    if (ok) errors = {};
  }

  function resolvedRedistributeArgs():
    { fromDate: string; targetDate: string; studyDays: number } | undefined {
    const effFrom = fromDate || today;
    const effTarget = targetDate || saved?.targetDate || '';
    if (!effTarget) {
      errors = { targetDate: m.course_schedule_error_target_required() };
      return undefined;
    }
    if (!validateWindow(effFrom, effTarget, studyDays, 'fromDate')) return undefined;
    return { fromDate: effFrom, targetDate: effTarget, studyDays };
  }

  async function redistributeMine(): Promise<void> {
    if (!course.canTrackProgress || busy) return;
    const args = resolvedRedistributeArgs();
    if (!args) return;
    const result = await course.schedule.redistributeMine(args);
    if (result.ok) {
      errors = {};
    } else if (result.error) {
      errors = fieldErrorsFromBackend(result.error);
    }
  }

  function askRedistributeShared(): void {
    if (!course.canEdit || busy) return;
    if (!resolvedRedistributeArgs()) return;
    confirmSharedOpen = true;
  }

  async function runRedistributeShared(): Promise<void> {
    const args = resolvedRedistributeArgs();
    if (!args) return;
    const result = await course.schedule.redistributeShared(args);
    if (result.ok) {
      errors = {};
    } else if (result.error) {
      errors = fieldErrorsFromBackend(result.error);
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    data-testid="course-schedule-dialog"
    showCloseButton={false}
    class={[
      'flex flex-col gap-0 p-0',
      compact
        ? 'top-0 left-0 h-dvh max-w-none translate-x-0 translate-y-0 rounded-none pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none'
        : 'left-[calc(50%+(var(--safe-left)-var(--safe-right))/2)] max-h-[calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard))] w-[min(32rem,calc(100%-2rem-var(--safe-left)-var(--safe-right)))] max-w-none sm:max-w-none',
    ]}
  >
    <Dialog.Header class="shrink-0 flex-row items-start gap-2 border-b py-2 pr-2 pl-4">
      <div class="min-w-0 flex-1 py-1">
        <Dialog.Title class="flex items-center gap-2 pr-0 text-base font-semibold">
          <CalendarClockIcon class="size-4 text-muted-foreground" aria-hidden="true" />
          {m.course_schedule_title()}
        </Dialog.Title>
        <Dialog.Description class="text-xs text-muted-foreground">
          {m.course_schedule_description()}
        </Dialog.Description>
      </div>
      <Dialog.Close>
        {#snippet child({ props }: { props: Record<string, unknown> })}
          <IconButton {...props} label={m.ui_close()} data-testid="close-course-schedule">
            <XIcon />
          </IconButton>
        {/snippet}
      </Dialog.Close>
    </Dialog.Header>

    <div class="grid min-h-0 flex-1 content-start gap-5 overflow-y-auto overscroll-contain p-4">
      <div
        class="flex flex-col gap-2 rounded-lg border bg-muted/40 p-3 text-xs"
        data-testid="schedule-dialog-summary"
      >
        <div class="flex flex-wrap items-center justify-between gap-2">
          <span class="text-muted-foreground">
            {m.course_schedule_today_label({ today, timeZone: course.workspace.regional.timeZone })}
          </span>
          <div class="flex items-center gap-1.5">
            <span
              class="rounded-md border border-primary/30 bg-primary/10 px-1.5 py-0.5 font-medium text-primary tabular-nums"
              data-testid="schedule-dialog-today-count"
            >
              {m.course_schedule_today_count({ count: todayCount })}
            </span>
            <span
              class={[
                'rounded-md border px-1.5 py-0.5 font-medium tabular-nums',
                overdueCount > 0
                  ? 'border-destructive/40 bg-destructive/10 text-destructive'
                  : 'border-border bg-background text-muted-foreground',
              ]}
              data-testid="schedule-dialog-overdue-count"
            >
              {m.course_schedule_overdue_count({ count: overdueCount })}
            </span>
          </div>
        </div>
        <p class="text-muted-foreground">{m.course_schedule_no_auto_shift()}</p>
      </div>

      {#if errors.general}
        <p class="text-xs text-destructive" role="alert" data-testid="schedule-error-general">
          {errors.general}
        </p>
      {/if}

      <form class="grid gap-4" onsubmit={(event) => void saveSchedule(event)}>
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {#if course.canEdit}
            <div class="grid gap-1.5">
              <Label for="{uid}-start">{m.course_schedule_start_date()}</Label>
              <Input
                id="{uid}-start"
                type="date"
                bind:value={startDate}
                disabled={busy}
                aria-invalid={errors.startDate ? 'true' : undefined}
                aria-describedby={errors.startDate ? `${uid}-start-err` : undefined}
                data-testid="schedule-start-date"
                class="roomy:h-touch"
              />
              {#if errors.startDate}
                <p
                  id="{uid}-start-err"
                  class="text-xs text-destructive"
                  role="alert"
                  data-testid="schedule-error-start"
                >
                  {errors.startDate}
                </p>
              {/if}
            </div>
          {/if}

          <div class="grid gap-1.5">
            <Label for="{uid}-target">{m.course_schedule_target_date()}</Label>
            <Input
              id="{uid}-target"
              type="date"
              bind:value={targetDate}
              disabled={busy}
              aria-invalid={errors.targetDate ? 'true' : undefined}
              aria-describedby={errors.targetDate ? `${uid}-target-err` : undefined}
              data-testid="schedule-target-date"
              class="roomy:h-touch"
            />
            {#if errors.targetDate}
              <p
                id="{uid}-target-err"
                class="text-xs text-destructive"
                role="alert"
                data-testid="schedule-error-target"
              >
                {errors.targetDate}
              </p>
            {/if}
          </div>
        </div>

        <fieldset
          class="grid min-w-0 gap-2"
          aria-describedby={errors.studyDays ? `${uid}-days-err` : undefined}
        >
          <legend class="text-sm font-medium">{m.course_schedule_study_days()}</legend>
          <div
            class="flex flex-wrap gap-1.5"
            role="group"
            aria-label={m.course_schedule_study_days()}
          >
            {#each STUDY_WEEKDAYS as day (day.key)}
              {@const checked = (studyDays & day.bit) !== 0}
              <button
                type="button"
                aria-pressed={checked}
                disabled={busy}
                data-testid="schedule-day-{day.key}"
                class={[
                  'min-h-8 min-w-10 rounded-md border px-2.5 text-xs font-medium transition-colors roomy:min-h-touch',
                  checked
                    ? 'border-primary bg-primary text-primary-foreground'
                    : 'border-border bg-background text-muted-foreground hover:bg-muted',
                ]}
                onclick={() => {
                  toggleDay(day.bit);
                }}
              >
                {weekdayLabel(day.key)}
              </button>
            {/each}
          </div>
          {#if errors.studyDays}
            <p
              id="{uid}-days-err"
              class="text-xs text-destructive"
              role="alert"
              data-testid="schedule-error-days"
            >
              {errors.studyDays}
            </p>
          {/if}
        </fieldset>

        {#if course.canEdit}
          <label class="flex cursor-pointer items-center gap-2 text-sm">
            <Checkbox
              bind:checked={autoAssign}
              disabled={busy}
              data-testid="schedule-auto-assign"
            />
            <span>{m.course_schedule_auto_assign()}</span>
          </label>

          <div class="flex flex-wrap items-center gap-2">
            <Button
              type="submit"
              size="sm"
              disabled={busy}
              data-testid="schedule-save"
              class="roomy:min-h-touch"
            >
              {m.course_schedule_save()}
            </Button>
            {#if saved}
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={busy}
                data-testid="schedule-delete"
                class="text-destructive hover:text-destructive roomy:min-h-touch"
                onclick={() => void clearSchedule()}
              >
                <Trash2Icon class="size-3.5" aria-hidden="true" />
                {m.course_schedule_delete()}
              </Button>
            {/if}
          </div>
          {#if saved}
            <p class="text-xs text-muted-foreground">{m.course_schedule_delete_hint()}</p>
          {/if}
        {/if}
      </form>

      <section class="grid gap-3 border-t pt-4" aria-labelledby="{uid}-redist">
        <h3 id="{uid}-redist" class="text-sm font-semibold">
          {m.course_schedule_redistribute_title()}
        </h3>

        <div class="grid gap-1.5">
          <Label for="{uid}-from">{m.course_schedule_redistribute_from()}</Label>
          <Input
            id="{uid}-from"
            type="date"
            bind:value={fromDate}
            disabled={busy}
            aria-invalid={errors.fromDate ? 'true' : undefined}
            aria-describedby={errors.fromDate ? `${uid}-from-err` : undefined}
            data-testid="schedule-from-date"
            class="sm:max-w-52 roomy:h-touch"
          />
          {#if errors.fromDate}
            <p
              id="{uid}-from-err"
              class="text-xs text-destructive"
              role="alert"
              data-testid="schedule-error-from"
            >
              {errors.fromDate}
            </p>
          {/if}
        </div>

        {#if course.canTrackProgress}
          <div class="grid gap-2 rounded-lg border bg-card p-3">
            <p class="text-xs text-muted-foreground">
              {m.course_schedule_redistribute_mine_hint()}
            </p>
            <div>
              <Button
                type="button"
                variant="secondary"
                size="sm"
                disabled={busy}
                data-testid="redistribute-my-schedule"
                class="roomy:min-h-touch"
                onclick={() => void redistributeMine()}
              >
                <RefreshCwIcon class="size-3.5" aria-hidden="true" />
                {m.course_schedule_redistribute_mine()}
              </Button>
            </div>
          </div>
        {/if}

        {#if course.canEdit}
          <div class="grid gap-2 rounded-lg border bg-card p-3">
            <p class="text-xs text-muted-foreground">
              {m.course_schedule_redistribute_shared_hint()}
            </p>
            <div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={busy}
                data-testid="redistribute-shared-schedule"
                class="roomy:min-h-touch"
                onclick={askRedistributeShared}
              >
                <RefreshCwIcon class="size-3.5" aria-hidden="true" />
                {m.course_schedule_redistribute_shared()}
              </Button>
            </div>
          </div>
        {/if}
      </section>
    </div>
  </Dialog.Content>
</Dialog.Root>

<ConfirmDialog
  bind:open={confirmSharedOpen}
  title={m.course_schedule_redistribute_shared_confirm_title()}
  description={m.course_schedule_redistribute_shared_confirm_body()}
  confirmLabel={m.course_schedule_redistribute_shared_confirm_action()}
  onConfirm={() => void runRedistributeShared()}
/>
