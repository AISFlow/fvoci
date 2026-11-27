<!--
  Personal completion toggle and optional last-position note for one unit.
  Available to every enrolled course member (including viewers and invited
  guests) and personal-space owners; nothing for a reader of an open course who
  has not joined it (the course's banner joins). Never mounted on public share
  links.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import BookmarkIcon from '@lucide/svelte/icons/bookmark';
  import CircleIcon from '@lucide/svelte/icons/circle';
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isComposing } from '$lib/keys.ts';
  import {
    courseReviewIntervals,
    courseReviews,
    DEFAULT_REVIEW_INTERVALS,
    hasControlChars,
    UNIT_PROGRESS_POSITION_MAX_CHARS,
    type CourseStore,
    type ScheduleUnitReview,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';

  import ReviewIntervalPicker from './ReviewIntervalPicker.svelte';

  interface Props {
    course: CourseStore;
    unitId: string;
  }

  let { course, unitId }: Props = $props();

  const uid = $props.id();
  const progress = $derived(course.progress.of(unitId));
  const completed = $derived(course.progress.isCompleted(unitId));
  const lastPosition = $derived(progress?.lastPosition ?? '');
  const busy = $derived(course.progress.isBusy(unitId));
  const canTrack = $derived(course.canTrackProgress);
  const reviewStore = $derived(courseReviews(course));
  const reviewIntervals = $derived(
    courseReviewIntervals(course.course?.properties as Record<string, unknown> | undefined),
  );

  let editingFor = $state<string | undefined>(undefined);
  let draft = $state('');
  let validationError = $state('');
  let inputRef = $state<HTMLInputElement | null>(null);
  let triggerRef = $state<HTMLElement | null>(null);
  const editingPosition = $derived(editingFor === unitId);

  let suggestFor = $state<string | undefined>(undefined);
  let suggestedDays = $state(DEFAULT_REVIEW_INTERVALS[0] ?? 1);
  let scheduledNotice = $state<{ unitId: string; text: string } | undefined>(undefined);
  const showingSuggestion = $derived(canTrack && completed && suggestFor === unitId);

  async function toggleComplete(): Promise<void> {
    const targetId = unitId;
    const wasCompleted = completed;
    scheduledNotice = undefined;
    const ok = await course.toggleUnitComplete(targetId);
    if (!ok || unitId !== targetId) return;
    if (!wasCompleted && course.progress.isCompleted(targetId)) {
      const nextDays =
        course.progress.of(targetId)?.suggestedNextIntervalDays ?? reviewIntervals[0] ?? 1;
      suggestedDays = nextDays;
      suggestFor = targetId;
    } else if (suggestFor === targetId) {
      suggestFor = undefined;
    }
  }

  async function scheduleSuggested(selection: ScheduleUnitReview): Promise<void> {
    const targetId = unitId;
    const created = await reviewStore.scheduleReview(targetId, selection);
    if (!created) return;
    if (suggestFor === targetId) suggestFor = undefined;
    scheduledNotice = {
      unitId: targetId,
      text: m.review_scheduled_status({ date: created.scheduledDate }),
    };
  }

  async function startEditing(): Promise<void> {
    if (!canTrack || busy) return;
    const targetId = unitId;
    draft = lastPosition;
    validationError = '';
    editingFor = targetId;
    await tick();
    if (editingFor === targetId) {
      inputRef?.focus();
      inputRef?.select();
    }
  }

  async function closeEditor(): Promise<void> {
    editingFor = undefined;
    validationError = '';
    await tick();
    triggerRef?.focus();
  }

  function validate(raw: string): string {
    if (hasControlChars(raw)) return m.course_last_position_invalid();
    if (Array.from(raw.trim()).length > UNIT_PROGRESS_POSITION_MAX_CHARS) {
      return m.course_last_position_too_long({ max: UNIT_PROGRESS_POSITION_MAX_CHARS });
    }
    return '';
  }

  async function submitPosition(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!canTrack || busy) return;
    const problem = validate(draft);
    if (problem !== '') {
      validationError = problem;
      return;
    }
    validationError = '';
    const targetId = unitId;
    const trimmed = draft.trim();
    if (trimmed === lastPosition) {
      await closeEditor();
      return;
    }
    const ok = await course.setUnitLastPosition(targetId, trimmed === '' ? null : trimmed);
    if (ok && editingFor === targetId) await closeEditor();
  }

  async function clearPosition(): Promise<void> {
    const targetId = unitId;
    const ok = await course.setUnitLastPosition(targetId, null);
    if (ok && unitId === targetId) {
      await tick();
      triggerRef?.focus();
    }
  }

  function onkeydown(event: KeyboardEvent): void {
    if (isComposing(event)) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      void closeEditor();
    }
  }
</script>

{#if canTrack}
  <div class="flex w-full flex-wrap items-center gap-2" data-testid="unit-progress-controls">
    <Button
      variant={completed ? 'default' : 'outline'}
      size="sm"
      class="roomy:min-h-touch"
      aria-pressed={completed}
      disabled={busy}
      data-testid="unit-complete-toggle"
      onclick={() => void toggleComplete()}
    >
      {#if completed}
        <CircleCheckIcon aria-hidden="true" />
        {m.course_unit_completed()}
      {:else}
        <CircleIcon aria-hidden="true" />
        {m.course_unit_mark_complete()}
      {/if}
    </Button>

    {#if editingPosition}
      <form
        class="flex min-w-0 flex-1 flex-wrap items-center gap-1.5"
        onsubmit={(event) => void submitPosition(event)}
        data-testid="unit-last-position-form"
      >
        <Input
          bind:ref={inputRef}
          type="text"
          bind:value={draft}
          aria-label={m.course_last_position_label()}
          aria-invalid={validationError !== '' ? 'true' : undefined}
          aria-describedby={validationError !== '' ? `${uid}-error` : undefined}
          placeholder={m.course_last_position_placeholder()}
          autocomplete="off"
          disabled={busy}
          data-testid="unit-last-position-input"
          class="h-8 max-w-60 min-w-36 flex-1 text-sm roomy:h-touch"
          oninput={() => {
            if (validationError !== '') validationError = validate(draft);
          }}
          {onkeydown}
        />
        <Button
          type="submit"
          size="sm"
          variant="secondary"
          class="roomy:min-h-touch"
          disabled={busy}
          data-testid="unit-last-position-save"
        >
          {m.course_last_position_save()}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          class="roomy:min-h-touch"
          disabled={busy}
          data-testid="unit-last-position-cancel"
          onclick={() => void closeEditor()}
        >
          {m.shared_cancel()}
        </Button>
        {#if validationError}
          <p
            id="{uid}-error"
            class="w-full text-xs text-destructive"
            role="alert"
            data-testid="unit-last-position-error"
          >
            {validationError}
          </p>
        {/if}
      </form>
    {:else if lastPosition}
      <div
        class="inline-flex max-w-full items-center gap-1 rounded-md border bg-muted/50 py-0.5 pr-1 pl-2 text-xs text-muted-foreground"
        data-testid="unit-last-position"
      >
        <button
          bind:this={triggerRef}
          type="button"
          class="flex min-h-7 min-w-0 items-center gap-1.5 truncate text-left hover:text-foreground roomy:min-h-touch"
          disabled={busy}
          data-testid="unit-last-position-edit"
          onclick={() => void startEditing()}
        >
          <BookmarkIcon class="size-3.5 shrink-0" aria-hidden="true" />
          <span class="sr-only">{m.course_last_position_edit()}: </span>
          <span class="truncate" data-testid="unit-last-position-value">{lastPosition}</span>
        </button>
        <IconButton
          label={m.course_last_position_clear()}
          disabled={busy}
          data-testid="unit-last-position-clear"
          class="size-6 shrink-0 text-muted-foreground hover:text-foreground"
          onclick={() => void clearPosition()}
        >
          <XIcon class="size-3.5" />
        </IconButton>
      </div>
    {:else}
      <Button
        bind:ref={triggerRef}
        variant="ghost"
        size="sm"
        class="text-muted-foreground roomy:min-h-touch"
        disabled={busy}
        data-testid="unit-last-position-add"
        onclick={() => void startEditing()}
      >
        <BookmarkIcon aria-hidden="true" />
        {m.course_last_position_add()}
      </Button>
    {/if}
  </div>
  {#if showingSuggestion}
    <ReviewIntervalPicker
      intervals={reviewIntervals}
      suggested={suggestedDays}
      title={m.review_suggest_title()}
      confirmLabel={m.review_suggest_confirm()}
      skipLabel={m.review_suggest_skip()}
      busy={reviewStore.reviewsBusy}
      testId="review-suggest-bar"
      onConfirm={scheduleSuggested}
      onSkip={() => {
        suggestFor = undefined;
      }}
    />
  {:else if scheduledNotice?.unitId === unitId}
    <p class="text-xs text-muted-foreground" role="status" data-testid="review-scheduled-notice">
      {scheduledNotice.text}
    </p>
  {/if}
{/if}
