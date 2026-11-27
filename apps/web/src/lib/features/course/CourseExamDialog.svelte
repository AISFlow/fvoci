<!--
  Modal dialog for creating or editing a course exam (editors/admins and
  personal-space owners). Supports tri-state optional `examDate` (`null` clears).
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import ClipboardPlusIcon from '@lucide/svelte/icons/clipboard-plus';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import { parseCalendarDate } from '$lib/logic/dates.ts';
  import {
    COURSE_EXAM_DESCRIPTION_MAX_CHARS,
    COURSE_EXAM_MAX_SCORE_LIMIT,
    COURSE_EXAM_TITLE_MAX_CHARS,
    hasControlChars,
    type CourseExam,
    type CourseStore,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { Textarea } from '$lib/ui/textarea/index.js';

  import { examErrorsFromBackend, type ExamFormErrors } from './course-meeting-labels.ts';

  interface Props {
    open: boolean;
    course: CourseStore;
    exam?: CourseExam | undefined;
  }

  let { open = $bindable(), course, exam }: Props = $props();

  const uid = $props.id();
  const compact = $derived(page.stage === 'compact');
  const editing = $derived(exam !== undefined);
  const busy = $derived(course.events.busy);

  let title = $state('');
  let examDate = $state('');
  let maxScoreText = $state<string | number | null>('100');
  let description = $state('');
  let errors = $state.raw<ExamFormErrors>({});
  let snapshotKey = '';

  $effect(() => {
    const key = open ? `${course.courseId}:${exam?.id ?? 'new'}` : '';
    if (key === snapshotKey) return;
    snapshotKey = key;
    if (!open) return;
    untrack(() => {
      title = exam?.title ?? '';
      examDate = exam?.examDate ?? '';
      maxScoreText = exam ? String(exam.maxScore) : '100';
      description = exam?.description ?? '';
      errors = {};
    });
  });

  closeOnBack(
    () => open,
    () => {
      open = false;
    },
  );

  function validate():
    { trimmedTitle: string; parsedDate: string | null; maxScore: number } | undefined {
    const next: ExamFormErrors = {};
    const trimmedTitle = title.trim();
    if (
      trimmedTitle === '' ||
      Array.from(trimmedTitle).length > COURSE_EXAM_TITLE_MAX_CHARS ||
      hasControlChars(trimmedTitle)
    ) {
      next.title = m.course_exams_error_title();
    }
    const trimmedDate = examDate.trim();
    const parsedDate = trimmedDate === '' ? null : (parseCalendarDate(trimmedDate) ?? undefined);
    if (trimmedDate !== '' && parsedDate === undefined) {
      next.examDate = m.course_exams_error_date();
    }
    const maxScore = Number(String(maxScoreText ?? '').trim());
    if (
      String(maxScoreText ?? '').trim() === '' ||
      !Number.isFinite(maxScore) ||
      maxScore <= 0 ||
      maxScore > COURSE_EXAM_MAX_SCORE_LIMIT
    ) {
      next.maxScore = m.course_exams_error_max_score();
    }
    if (Array.from(description).length > COURSE_EXAM_DESCRIPTION_MAX_CHARS) {
      next.description = m.course_exams_error_description();
    }
    if (Object.keys(next).length > 0 || parsedDate === undefined) {
      errors = next;
      return undefined;
    }
    errors = {};
    return { trimmedTitle, parsedDate, maxScore };
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!course.canEdit || busy) return;
    const valid = validate();
    if (!valid) return;
    const payload = {
      title: valid.trimmedTitle,
      examDate: valid.parsedDate,
      maxScore: valid.maxScore,
      description,
    };
    const result = exam
      ? await course.events.patchExam(exam.id, payload)
      : await course.events.createExam(payload);
    if (result.ok) {
      open = false;
    } else if (result.error) {
      errors = examErrorsFromBackend(result.error);
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    showCloseButton={false}
    data-testid="course-exam-dialog"
    class={compact
      ? 'top-0 left-0 flex h-dvh max-w-none translate-x-0 translate-y-0 flex-col gap-0 rounded-none p-0 pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none'
      : 'flex max-h-[min(40rem,calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard)))] flex-col gap-0 p-0 sm:max-w-md'}
  >
    <Dialog.Header class="shrink-0 flex-row items-center gap-2 border-b px-4 py-3">
      <ClipboardPlusIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      <Dialog.Title class="min-w-0 flex-1 text-base font-semibold">
        {editing ? m.course_exams_edit() : m.course_exams_new()}
      </Dialog.Title>
      <Dialog.Close>
        {#snippet child({ props }: { props: Record<string, unknown> })}
          <IconButton {...props} label={m.ui_close()} data-testid="close-exam-dialog">
            <XIcon />
          </IconButton>
        {/snippet}
      </Dialog.Close>
    </Dialog.Header>

    <form novalidate class="flex min-h-0 flex-1 flex-col" onsubmit={(e) => void submit(e)}>
      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 py-4">
        <div class="flex flex-col gap-1">
          <Label for="{uid}-title">{m.course_exams_field_title()}</Label>
          <Input
            id="{uid}-title"
            bind:value={title}
            maxlength={COURSE_EXAM_TITLE_MAX_CHARS}
            required
            disabled={busy}
            data-testid="exam-title-input"
            class="roomy:min-h-touch"
          />
          {#if errors.title}
            <p class="text-xs text-destructive" role="alert">{errors.title}</p>
          {/if}
        </div>

        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div class="flex flex-col gap-1">
            <Label for="{uid}-date">{m.course_exams_field_date()}</Label>
            <div class="flex items-center gap-1">
              <Input
                id="{uid}-date"
                type="date"
                bind:value={examDate}
                disabled={busy}
                data-testid="exam-date-input"
                class="min-w-0 flex-1 roomy:min-h-touch"
              />
              {#if examDate}
                <IconButton
                  type="button"
                  label={m.course_exams_field_date_clear()}
                  disabled={busy}
                  data-testid="exam-date-clear"
                  onclick={() => {
                    examDate = '';
                  }}
                >
                  <XIcon />
                </IconButton>
              {/if}
            </div>
            {#if errors.examDate}
              <p class="text-xs text-destructive" role="alert">{errors.examDate}</p>
            {/if}
          </div>

          <div class="flex flex-col gap-1">
            <Label for="{uid}-max">{m.course_exams_field_max_score()}</Label>
            <Input
              id="{uid}-max"
              type="number"
              inputmode="decimal"
              min="0.01"
              max={COURSE_EXAM_MAX_SCORE_LIMIT}
              step="any"
              bind:value={maxScoreText}
              required
              disabled={busy}
              data-testid="exam-max-score-input"
              class="tabular-nums roomy:min-h-touch"
            />
            {#if errors.maxScore}
              <p class="text-xs text-destructive" role="alert">{errors.maxScore}</p>
            {/if}
          </div>
        </div>

        <div class="flex flex-col gap-1">
          <Label for="{uid}-desc">{m.course_exams_field_description()}</Label>
          <Textarea
            id="{uid}-desc"
            rows={3}
            bind:value={description}
            maxlength={COURSE_EXAM_DESCRIPTION_MAX_CHARS}
            disabled={busy}
            data-testid="exam-description-input"
          />
          {#if errors.description}
            <p class="text-xs text-destructive" role="alert">{errors.description}</p>
          {/if}
        </div>

        {#if errors.general}
          <p class="text-xs text-destructive" role="alert">{errors.general}</p>
        {/if}
      </div>

      <Dialog.Footer class="shrink-0 border-t px-4 py-3">
        <Button
          type="button"
          variant="outline"
          class="roomy:min-h-touch"
          disabled={busy}
          onclick={() => {
            open = false;
          }}
        >
          {m.shared_cancel()}
        </Button>
        <Button type="submit" class="roomy:min-h-touch" disabled={busy} data-testid="exam-submit">
          {editing ? m.course_exams_save() : m.course_exams_create()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
