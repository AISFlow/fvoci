<!--
  Exams tab of a course: exam list ordered by `examDate ASC NULLS LAST`,
  what is told of everybody's scores (`ExamResults`: the published snapshot
  and how many scored), editor CRUD dialog, and strictly personal score editor
  (`ExamMyScorePanel`).

  Privacy invariant: no other member's individual score UI ever exists here,
  and this tab is never mounted on public share links (`/shared/...`).
-->
<script lang="ts">
  import CalendarIcon from '@lucide/svelte/icons/calendar';
  import LockIcon from '@lucide/svelte/icons/lock';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { parseCalendarDate } from '$lib/logic/dates.ts';
  import { displayCalendarDate } from '$lib/logic/format.ts';
  import type { CourseExam, CourseStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import CourseExamDialog from './CourseExamDialog.svelte';
  import ExamMyScorePanel from './ExamMyScorePanel.svelte';
  import ExamResults from './ExamResults.svelte';
  import { formatScore } from './course-meeting-labels.ts';

  interface Props {
    course: CourseStore;
  }

  let { course }: Props = $props();

  let dialogOpen = $state(false);
  let editingExam = $state.raw<CourseExam | undefined>(undefined);
  let deletingExam = $state.raw<CourseExam | undefined>(undefined);
  let deleteOpen = $state(false);

  const exams = $derived(course.events.exams);
  const locale = $derived(course.workspace.locale);
  const displayContext = $derived({
    locale: course.workspace.locale,
    timeZone: course.workspace.regional.timeZone,
  });

  function formatExamDate(raw: string | null): string {
    if (!raw) return m.course_exams_date_unset();
    const parsed = parseCalendarDate(raw);
    return parsed ? displayCalendarDate(parsed, displayContext) : raw;
  }

  function openCreate(): void {
    editingExam = undefined;
    dialogOpen = true;
  }

  function openEdit(exam: CourseExam): void {
    editingExam = exam;
    dialogOpen = true;
  }

  function askDelete(exam: CourseExam): void {
    deletingExam = exam;
    deleteOpen = true;
  }
</script>

<div class="mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6" data-testid="course-exams-tab">
  <header class="flex flex-wrap items-start justify-between gap-3">
    <div class="min-w-0 flex-1">
      <h2 class="text-xl font-semibold">{m.course_exams_title()}</h2>
      <p class="mt-0.5 text-sm text-muted-foreground">{m.course_exams_subtitle()}</p>
    </div>
    {#if course.canEdit}
      <Button class="shrink-0 roomy:min-h-touch" data-testid="new-exam" onclick={openCreate}>
        <PlusIcon aria-hidden="true" />
        {m.course_exams_new()}
      </Button>
    {/if}
  </header>

  <div
    class="flex items-start gap-2 rounded-lg border bg-muted/60 px-3 py-2.5 text-sm text-muted-foreground"
    data-testid="exams-privacy-notice"
  >
    <LockIcon class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
    <p>{m.course_exams_privacy_notice()}</p>
  </div>

  {#if course.events.examsFailed && !course.events.examsLoaded}
    <div class="flex flex-col items-center gap-3 py-8 text-center">
      <p class="text-sm text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      <Button
        variant="outline"
        size="sm"
        class="roomy:min-h-touch"
        onclick={() => void course.events.refreshExams()}
      >
        {m.app_retry()}
      </Button>
    </div>
  {:else if !course.events.examsLoaded}
    <p class="py-8 text-center text-sm text-muted-foreground" role="status">{m.app_loading()}</p>
  {:else if exams.length === 0}
    <div
      class="flex flex-col items-center gap-3 rounded-lg border border-dashed px-6 py-10 text-center"
      data-testid="exams-empty"
    >
      <p class="font-medium">{m.course_exams_empty()}</p>
      <p class="max-w-md text-sm text-muted-foreground">{m.course_exams_empty_hint()}</p>
      {#if course.canEdit}
        <Button class="roomy:min-h-touch" onclick={openCreate}>
          <PlusIcon aria-hidden="true" />
          {m.course_exams_new()}
        </Button>
      {/if}
    </div>
  {:else}
    <div class="flex flex-col gap-4">
      {#each exams as exam (exam.id)}
        {@const maxLabel = formatScore(exam.maxScore, locale)}
        <article
          class="flex flex-col gap-3 rounded-lg border bg-card p-4 text-card-foreground"
          data-testid="exam-card"
          data-exam-id={exam.id}
        >
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0 flex-1">
              <h3 class="text-base font-semibold break-words" data-testid="exam-title">
                {exam.title}
              </h3>
              <div
                class="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground"
              >
                <span class="inline-flex items-center gap-1" data-testid="exam-date">
                  <CalendarIcon class="size-3.5 shrink-0" aria-hidden="true" />
                  {formatExamDate(exam.examDate)}
                </span>
                <span class="tabular-nums" data-testid="exam-max-score">
                  {m.course_exams_max_score({ max: maxLabel })}
                </span>
              </div>
            </div>

            {#if course.canEdit}
              <div class="flex shrink-0 items-center gap-1">
                <IconButton
                  label={m.course_exams_edit()}
                  data-testid="edit-exam"
                  class="text-muted-foreground"
                  onclick={() => {
                    openEdit(exam);
                  }}
                >
                  <PencilIcon />
                </IconButton>
                <IconButton
                  label={m.course_exams_delete()}
                  data-testid="delete-exam"
                  class="text-muted-foreground hover:text-destructive"
                  onclick={() => {
                    askDelete(exam);
                  }}
                >
                  <Trash2Icon />
                </IconButton>
              </div>
            {/if}
          </div>

          {#if exam.description}
            <p
              class="text-sm whitespace-pre-line text-muted-foreground"
              data-testid="exam-description"
            >
              {exam.description}
            </p>
          {/if}

          <ExamResults {course} {exam} />

          {#if course.canTrackProgress}
            <ExamMyScorePanel {course} {exam} />
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</div>

{#if course.canEdit}
  <CourseExamDialog bind:open={dialogOpen} {course} exam={editingExam} />
  {#if deletingExam}
    {@const target = deletingExam}
    <ConfirmDialog
      bind:open={deleteOpen}
      title={m.course_exams_delete_title()}
      description={m.course_exams_delete_info({ title: target.title })}
      confirmLabel={m.shared_delete()}
      destructive
      onConfirm={() => {
        void course.events.deleteExam(target.id);
      }}
    />
  {/if}
{/if}
