<!--
  What an exam's card says of everybody's scores (`examAverage`): whether the
  results are published ("결과 공개", with when), the course average of the
  published snapshot with how many had scored then ("5명+"), and how many
  have scored: exactly for who manages the course, else counted down to fives
  ("5명 이상"). Never anybody's own score but the reader's, which is
  `ExamMyScorePanel`'s.

  Who manages the course publishes the snapshot from here once five have
  scored, and takes it back; the server refuses one that changed too little
  since the last, which a line says.
-->
<script lang="ts">
  import BarChart3Icon from '@lucide/svelte/icons/bar-chart-3';
  import MegaphoneIcon from '@lucide/svelte/icons/megaphone';
  import UsersIcon from '@lucide/svelte/icons/users';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { examAverage, examScoreCount, FEWEST_PUBLISHED_SCORES } from '$lib/logic/exam-results.ts';
  import { displayDate } from '$lib/logic/format.ts';
  import type { CourseExam, CourseStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { formatScore } from './course-meeting-labels.ts';

  let { course, exam }: { course: CourseStore; exam: CourseExam } = $props();

  const uid = $props.id();
  const locale = $derived(course.workspace.locale);
  const manages = $derived(course.permissions.canManageRoles);
  const average = $derived(examAverage(exam));
  const scored = $derived(examScoreCount(exam, manages));
  const maxLabel = $derived(formatScore(exam.maxScore, locale));
  const publishedOn = $derived(
    exam.resultsPublishedAt === null
      ? ''
      : displayDate(exam.resultsPublishedAt, {
          locale,
          timeZone: course.workspace.regional.timeZone,
        }),
  );
  const tooFew = $derived(
    average.kind !== 'published' && exam.scoreCount < FEWEST_PUBLISHED_SCORES,
  );
  let refused = $state(false);

  async function publish(published: boolean): Promise<void> {
    refused = false;
    refused = (await course.events.setResultsPublished(exam.id, published)) === 'refused';
  }

  const BADGE =
    'inline-flex items-center gap-1.5 rounded-md border bg-muted/60 px-2.5 py-1 text-xs font-medium tabular-nums';
</script>

<div class="flex flex-wrap items-center gap-2" data-testid="exam-results">
  {#if average.kind === 'published'}
    <span
      class="inline-flex items-center gap-1.5 rounded-md border border-primary/40 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary"
      title={m.course_exams_published_on({ date: publishedOn })}
      data-testid="exam-results-published"
    >
      <MegaphoneIcon class="size-3.5" aria-hidden="true" />
      {m.course_exams_results_published()}
    </span>
  {/if}
  <span class={BADGE} data-testid="exam-average">
    <BarChart3Icon class="size-3.5 text-muted-foreground" aria-hidden="true" />
    <span class="text-muted-foreground">{m.course_exams_average()}:</span>
    <span>
      {#if average.kind === 'published' || average.kind === 'shown'}
        {m.course_exams_average_value({
          average: formatScore(average.average, locale),
          max: maxLabel,
        })}
      {:else if average.kind === 'held'}
        {m.course_exams_average_hidden()}
      {:else}
        {m.course_exams_average_empty()}
      {/if}
    </span>
  </span>
  {#if average.kind === 'published' && average.atLeast}
    <span class={[BADGE, 'text-muted-foreground']} data-testid="exam-published-bucket">
      {m.course_exams_published_bucket({ count: average.atLeast })}
    </span>
  {/if}
  <span class={[BADGE, 'text-muted-foreground']} data-testid="exam-score-count">
    <UsersIcon class="size-3.5" aria-hidden="true" />
    {scored.atLeast
      ? m.course_exams_score_count_at_least({ count: scored.count })
      : m.course_exams_score_count({ count: scored.count })}
  </span>
  {#if manages}
    <Button
      variant="outline"
      size="sm"
      class="roomy:min-h-touch"
      disabled={course.events.busy || tooFew}
      aria-describedby={tooFew ? `${uid}-few` : undefined}
      data-testid="exam-publish"
      onclick={() => void publish(average.kind !== 'published')}
    >
      {average.kind === 'published' ? m.course_exams_unpublish() : m.course_exams_publish()}
    </Button>
    {#if tooFew}
      <span id="{uid}-few" class="text-xs text-muted-foreground">
        {m.course_exams_publish_few({ count: FEWEST_PUBLISHED_SCORES })}
      </span>
    {/if}
  {/if}
</div>
{#if refused}
  <p class="text-sm text-destructive" role="alert" data-testid="exam-publish-refused">
    {m.course_exams_publish_refused()}
  </p>
{/if}
