<!--
  Strictly personal exam score editor for the authenticated enrolled caller.
  Never displays or requests any other member's score. Saving or deleting a
  score re-fetches the exam list so the aggregate course average and count
  update immediately.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import LockIcon from '@lucide/svelte/icons/lock';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    EXAM_SCORE_NOTE_MAX_CHARS,
    type CourseExam,
    type CourseStore,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { Textarea } from '$lib/ui/textarea/index.js';

  import { formatScore } from './course-meeting-labels.ts';

  interface Props {
    course: CourseStore;
    exam: CourseExam;
  }

  let { course, exam }: Props = $props();

  const uid = $props.id();
  const busy = $derived(course.events.isExamScoreBusy(exam.id));
  const locale = $derived(course.workspace.locale);
  const maxLabel = $derived(formatScore(exam.maxScore, locale));

  let scoreText = $state<string | number | null>('');
  let noteText = $state('');
  let scoreError = $state<string | undefined>(undefined);
  let noteError = $state<string | undefined>(undefined);
  let lastSavedStamp = '';

  $effect(() => {
    const stamp = `${exam.id}:${exam.myScore?.updatedAt ?? 'none'}:${exam.myScore?.score ?? ''}:${exam.myScore?.note ?? ''}`;
    if (stamp === lastSavedStamp) return;
    lastSavedStamp = stamp;
    untrack(() => {
      scoreText = exam.myScore ? String(exam.myScore.score) : '';
      noteText = exam.myScore?.note ?? '';
      scoreError = undefined;
      noteError = undefined;
    });
  });

  async function save(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busy) return;
    scoreError = undefined;
    noteError = undefined;

    const trimmed = String(scoreText ?? '').trim();
    const numeric = Number(trimmed);
    if (trimmed === '' || !Number.isFinite(numeric) || numeric < 0 || numeric > exam.maxScore) {
      scoreError = m.course_exams_error_score({ max: maxLabel });
      return;
    }
    if (Array.from(noteText).length > EXAM_SCORE_NOTE_MAX_CHARS) {
      noteError = m.course_exams_error_note();
      return;
    }
    const result = await course.events.saveMyScore(exam.id, {
      score: numeric,
      note: noteText,
    });
    if (!result.ok && result.error) {
      if (result.error.refusalAt('/score')) {
        scoreError = m.course_exams_error_score({ max: maxLabel });
      }
      if (result.error.refusalAt('/note')) noteError = m.course_exams_error_note();
      if (!scoreError && !noteError) {
        scoreError = m.app_problem_save();
      }
    }
  }

  async function remove(): Promise<void> {
    if (busy || !exam.myScore) return;
    const ok = await course.events.deleteMyScore(exam.id);
    if (ok) {
      scoreText = '';
      noteText = '';
      scoreError = undefined;
      noteError = undefined;
    }
  }
</script>

<form
  novalidate
  class="flex flex-col gap-3 rounded-md border bg-muted/40 p-3"
  data-testid="exam-my-score-panel"
  onsubmit={(e) => void save(e)}
>
  <div class="flex flex-wrap items-center justify-between gap-2">
    <h4 class="inline-flex items-center gap-1.5 text-xs font-semibold text-foreground">
      <LockIcon class="size-3.5 text-muted-foreground" aria-hidden="true" />
      {m.course_exams_my_score_title()}
    </h4>
    {#if exam.myScore}
      <span
        class="rounded-full bg-primary/10 px-2 py-0.5 text-xs font-medium text-primary tabular-nums"
        data-testid="exam-my-score-saved"
      >
        {m.course_exams_my_score_saved({
          score: formatScore(exam.myScore.score, locale),
          max: maxLabel,
        })}
      </span>
    {/if}
  </div>

  <div class="grid grid-cols-1 gap-3 sm:grid-cols-[10rem_1fr]">
    <div class="flex flex-col gap-1">
      <Label for="{uid}-score" class="text-xs">
        {m.course_exams_my_score_label({ max: maxLabel })}
      </Label>
      <Input
        id="{uid}-score"
        type="number"
        inputmode="decimal"
        min="0"
        max={exam.maxScore}
        step="any"
        bind:value={scoreText}
        disabled={busy}
        data-testid="exam-my-score-input"
        class="bg-background tabular-nums roomy:min-h-touch"
      />
      {#if scoreError}
        <p class="text-xs text-destructive" role="alert" data-testid="exam-my-score-error">
          {scoreError}
        </p>
      {/if}
    </div>

    <div class="flex flex-col gap-1">
      <Label for="{uid}-note" class="text-xs">{m.course_exams_my_note_label()}</Label>
      <Textarea
        id="{uid}-note"
        rows={2}
        bind:value={noteText}
        maxlength={EXAM_SCORE_NOTE_MAX_CHARS}
        placeholder={m.course_exams_my_note_placeholder()}
        disabled={busy}
        data-testid="exam-my-note-input"
        class="bg-background"
      />
      {#if noteError}
        <p class="text-xs text-destructive" role="alert">{noteError}</p>
      {/if}
    </div>
  </div>

  <div class="flex flex-wrap items-center justify-end gap-2">
    {#if exam.myScore}
      <Button
        type="button"
        variant="outline"
        size="sm"
        class="text-destructive hover:text-destructive roomy:min-h-touch"
        disabled={busy}
        data-testid="exam-my-score-delete"
        onclick={() => void remove()}
      >
        <Trash2Icon class="size-3.5" aria-hidden="true" />
        {m.course_exams_my_score_delete()}
      </Button>
    {/if}
    <Button
      type="submit"
      size="sm"
      class="roomy:min-h-touch"
      disabled={busy}
      data-testid="exam-my-score-save"
    >
      {m.course_exams_my_score_save()}
    </Button>
  </div>
</form>
