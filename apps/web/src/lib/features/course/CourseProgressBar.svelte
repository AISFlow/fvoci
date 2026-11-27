<!--
  The learner's own progress bar in the course header: shows percentage and
  completed/total live units for enrolled members and personal-space owners, or
  a subtle prompt to join for unjoined open-course readers.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CourseStore } from '$lib/state/index.ts';

  interface Props {
    course: CourseStore;
  }

  let { course }: Props = $props();

  const total = $derived(course.progress.totalUnits);
  const completed = $derived(course.progress.completedUnits);
  const percent = $derived(course.progress.percent);
  const summaryText = $derived(m.course_progress_rate({ percent, completed, total }));
</script>

{#if course.canTrackProgress}
  <div class="flex flex-col gap-1.5 pt-1" data-testid="course-my-progress">
    <div class="flex items-baseline justify-between gap-2 text-xs">
      <span class="font-medium text-muted-foreground">{m.course_my_progress()}</span>
      <span class="font-medium text-foreground tabular-nums" data-testid="course-progress-text">
        {summaryText}
      </span>
    </div>
    <div
      role="progressbar"
      aria-label={m.course_my_progress()}
      aria-valuemin={0}
      aria-valuemax={total}
      aria-valuenow={completed}
      aria-valuetext={summaryText}
      data-testid="course-progress-bar"
      class="h-1.5 w-full overflow-hidden rounded-full bg-muted"
    >
      <div
        class="h-full rounded-full bg-primary transition-[width] duration-200 motion-reduce:transition-none"
        style:width="{percent}%"
      ></div>
    </div>
  </div>
{/if}
