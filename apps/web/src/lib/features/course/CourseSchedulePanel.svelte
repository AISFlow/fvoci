<!--
  Compact study-schedule summary in the course left column: shows the saved
  date range (if any), today/overdue unit counts, an explicit reminder that
  overdue units never shift automatically, and a button to open the schedule
  & manual redistribution dialog. Never mounted on public share links.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import CalendarIcon from '@lucide/svelte/icons/calendar';
  import CalendarClockIcon from '@lucide/svelte/icons/calendar-clock';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CourseStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import CourseScheduleDialog from './CourseScheduleDialog.svelte';

  interface Props {
    course: CourseStore;
  }

  let { course }: Props = $props();

  let dialogOpen = $state(false);
  let lastZone: string | undefined;

  // In another time zone, today may be another day: the schedule is read anew.
  $effect(() => {
    const zone = course.workspace.settings.timeZone;
    untrack(() => {
      if (lastZone !== undefined && lastZone !== zone) void course.schedule.refresh();
      lastZone = zone;
    });
  });

  const schedule = $derived(course.schedule.schedule);
  const todayCount = $derived(course.schedule.todayCount);
  const overdueCount = $derived(course.schedule.overdueCount);
  const canOpenDialog = $derived(course.canEdit || course.canTrackProgress);
  const showPanel = $derived(
    canOpenDialog || schedule !== null || todayCount > 0 || overdueCount > 0,
  );
</script>

{#if showPanel}
  <div class="flex flex-col gap-1.5 border-t pt-2 text-xs" data-testid="course-schedule-panel">
    <div class="flex items-center justify-between gap-2">
      <span class="flex min-w-0 items-center gap-1.5 font-medium text-muted-foreground">
        <CalendarIcon class="size-3.5 shrink-0" aria-hidden="true" />
        <span class="truncate" data-testid="course-schedule-range">
          {#if schedule}
            {m.course_schedule_range({ start: schedule.startDate, target: schedule.targetDate })}
          {:else}
            {m.course_schedule_none()}
          {/if}
        </span>
      </span>
      {#if canOpenDialog}
        <Button
          variant="outline"
          size="sm"
          class="h-6 shrink-0 px-2 text-xs roomy:min-h-touch"
          aria-haspopup="dialog"
          data-testid="open-course-schedule"
          onclick={() => {
            dialogOpen = true;
          }}
        >
          <CalendarClockIcon class="size-3.5" aria-hidden="true" />
          {course.canEdit ? m.course_schedule_open() : m.course_schedule_open_mine()}
        </Button>
      {/if}
    </div>

    <div class="flex flex-wrap items-center gap-1.5">
      <span
        class={[
          'inline-flex items-center rounded-md border px-1.5 py-0.5 text-xs font-medium tabular-nums',
          todayCount > 0
            ? 'border-primary/30 bg-primary/10 text-primary'
            : 'border-border bg-muted/60 text-muted-foreground',
        ]}
        data-testid="course-schedule-today-count"
      >
        {m.course_schedule_today_count({ count: todayCount })}
      </span>
      <span
        class={[
          'inline-flex items-center rounded-md border px-1.5 py-0.5 text-xs font-medium tabular-nums',
          overdueCount > 0
            ? 'border-destructive/40 bg-destructive/10 text-destructive'
            : 'border-border bg-muted/60 text-muted-foreground',
        ]}
        data-testid="course-schedule-overdue-count"
      >
        {m.course_schedule_overdue_count({ count: overdueCount })}
      </span>
    </div>

    {#if overdueCount > 0}
      <p
        class="text-[0.6875rem] leading-snug text-destructive"
        data-testid="course-schedule-no-auto-shift"
      >
        {m.course_schedule_no_auto_shift()}
      </p>
    {/if}
  </div>

  {#if canOpenDialog}
    <CourseScheduleDialog bind:open={dialogOpen} {course} />
  {/if}
{/if}
