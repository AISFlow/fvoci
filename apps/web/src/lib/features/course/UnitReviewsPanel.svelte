<!--
  Personal spaced-repetition review schedule panel for one course unit, shown
  once the unit has a review of the caller's or one is being planned (from the
  unit's menu, `openings.review`). Shows pending and completed reviews, allows
  manual scheduling, completing a review (with optional atomic follow-up
  scheduling), and deleting.
  Nothing for who cannot keep reviews (`canTrackProgress`: a reader of an open
  course who has not joined it joins by the course's banner); never mounted on
  public links or chapters.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import CheckIcon from '@lucide/svelte/icons/check';
  import LockIcon from '@lucide/svelte/icons/lock';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { daysBetween, parseCalendarDate } from '$lib/logic/dates.ts';
  import {
    courseReviewIntervals,
    courseReviews,
    DeviceToday,
    suggestedNextIntervalDays,
    type CourseStore,
    type ScheduleUnitReview,
    type UnitReview,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import ReviewIntervalPicker from './ReviewIntervalPicker.svelte';
  import type { UnitOpenings } from './unit-openings.svelte.ts';

  interface Props {
    course: CourseStore;
    unitId: string;
    openings: UnitOpenings;
  }

  let { course, unitId, openings }: Props = $props();

  const store = $derived(courseReviews(course));
  const canTrack = $derived(course.canTrackProgress);
  const intervals = $derived(
    courseReviewIntervals(course.course?.properties as Record<string, unknown> | undefined),
  );
  const todayClock = new DeviceToday(() => course.workspace.regional.timeZone);
  const today = $derived(todayClock.date);

  const unitReviews = $derived(store.reviewsForUnit(unitId));
  const pendingReviews = $derived(unitReviews.filter((review) => review.completedAt === null));
  const completedReviews = $derived(unitReviews.filter((review) => review.completedAt !== null));
  const nextSuggested = $derived.by(() => {
    const latest = [...unitReviews].sort((a, b) => {
      const aOrder = a.completedAt ?? a.createdAt;
      const bOrder = b.completedAt ?? b.createdAt;
      if (aOrder !== bOrder) return bOrder - aOrder;
      if (a.createdAt !== b.createdAt) return b.createdAt - a.createdAt;
      return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
    })[0];
    return suggestedNextIntervalDays(latest?.intervalDays ?? null, intervals);
  });

  let completingId = $state<string | undefined>(undefined);
  let statusMessage = $state<{ unitId: string; text: string } | undefined>(undefined);
  const scheduling = $derived(openings.review === unitId);

  $effect(() => {
    const active = canTrack;
    const current = store;
    untrack(() => {
      if (active) {
        if (current.reviewsStatus === 'idle') void current.loadReviews();
      } else {
        current.clear();
      }
    });
  });

  function overdueDaysOf(review: UnitReview): number {
    const parsed = parseCalendarDate(review.scheduledDate);
    if (!parsed) return 0;
    return Math.max(0, daysBetween(parsed, today));
  }

  async function scheduleManual(selection: ScheduleUnitReview): Promise<void> {
    const targetId = unitId;
    const created = await store.scheduleReview(targetId, selection);
    if (!created) return;
    if (openings.review === targetId) openings.review = undefined;
    statusMessage = {
      unitId: targetId,
      text: m.review_scheduled_status({ date: created.scheduledDate }),
    };
  }

  async function completeWithNext(
    review: UnitReview,
    selection: ScheduleUnitReview,
  ): Promise<void> {
    const targetId = unitId;
    const nextDays = selection.intervalDays;
    const result = await store.completeReview(
      review.id,
      nextDays !== undefined ? { scheduleNextIntervalDays: nextDays } : {},
    );
    if (!result) return;
    if (completingId === review.id) completingId = undefined;
    statusMessage = {
      unitId: targetId,
      text: result.nextReview
        ? m.review_completed_next_status({ date: result.nextReview.scheduledDate })
        : m.review_completed_status(),
    };
  }

  async function completeOnly(review: UnitReview): Promise<void> {
    const targetId = unitId;
    const result = await store.completeReview(review.id, {});
    if (!result) return;
    if (completingId === review.id) completingId = undefined;
    statusMessage = { unitId: targetId, text: m.review_completed_status() };
  }
</script>

{#if canTrack && (unitReviews.length > 0 || scheduling || store.reviewsStatus === 'failed')}
  <section
    class="flex flex-col gap-3 border-t pt-5"
    aria-label={m.review_panel_title()}
    data-testid="unit-reviews-panel"
  >
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <h2 class="flex items-center gap-1.5 text-base font-semibold">
          <CalendarCheckIcon class="size-4 text-muted-foreground" aria-hidden="true" />
          {m.review_panel_title()}
        </h2>
        <span
          class="inline-flex items-center gap-1 rounded-full border bg-muted/60 px-2 py-0.5 text-xs text-muted-foreground"
        >
          <LockIcon class="size-3" aria-hidden="true" />
          {m.review_panel_private()}
        </span>
      </div>
      <Button
        variant="outline"
        size="sm"
        class="roomy:min-h-touch"
        data-testid="unit-review-add-toggle"
        aria-expanded={scheduling}
        onclick={() => {
          openings.review = scheduling ? undefined : unitId;
        }}
      >
        <PlusIcon class="size-3.5" aria-hidden="true" />
        {m.review_panel_add()}
      </Button>
    </div>

    {#if scheduling}
      <ReviewIntervalPicker
        {intervals}
        suggested={nextSuggested}
        title={m.review_panel_add()}
        confirmLabel={m.review_suggest_confirm()}
        skipLabel={m.shared_cancel()}
        allowDate
        busy={store.reviewsBusy}
        testId="unit-review-manual-picker"
        onConfirm={scheduleManual}
        onSkip={() => {
          openings.review = undefined;
        }}
      />
    {/if}

    {#if statusMessage?.unitId === unitId}
      <p class="text-xs text-muted-foreground" role="status" data-testid="unit-reviews-status">
        {statusMessage.text}
      </p>
    {/if}

    {#if store.reviewsStatus === 'loading' && unitReviews.length === 0}
      <p class="text-sm text-muted-foreground" role="status">{m.app_loading()}</p>
    {:else if store.reviewsStatus === 'failed'}
      <div class="flex items-center gap-2" role="alert">
        <p class="text-sm text-muted-foreground">{m.app_problem_load()}</p>
        <Button
          variant="outline"
          size="sm"
          class="roomy:min-h-touch"
          onclick={() => void store.loadReviews()}
        >
          {m.app_retry()}
        </Button>
      </div>
    {:else}
      {#if pendingReviews.length > 0}
        <div class="flex flex-col gap-2" data-testid="unit-reviews-pending">
          <h3 class="text-xs font-medium text-muted-foreground">{m.review_panel_pending()}</h3>
          <ul class="flex flex-col gap-2">
            {#each pendingReviews as review (review.id)}
              {@const overdue = overdueDaysOf(review)}
              {@const isDueToday = review.scheduledDate === today}
              {@const busy = store.reviewBusyIds.has(review.id)}
              {@const expanding = completingId === review.id}
              <li
                class="flex flex-col gap-2 rounded-lg border bg-background p-3"
                data-testid="unit-review-item"
                data-review-id={review.id}
                data-review-status="pending"
              >
                <div class="flex flex-wrap items-center justify-between gap-2">
                  <div class="flex min-w-0 flex-wrap items-center gap-2">
                    <span class="text-sm font-medium tabular-nums">{review.scheduledDate}</span>
                    <span
                      class="rounded-full border bg-muted/50 px-2 py-0.5 text-xs text-muted-foreground"
                    >
                      {m.review_interval_badge({ days: review.intervalDays })}
                    </span>
                    {#if overdue > 0}
                      <span
                        class="rounded-full border border-destructive/30 bg-destructive/10 px-2 py-0.5 text-xs font-medium text-destructive"
                        data-testid="unit-review-overdue"
                      >
                        {m.review_overdue_days({ days: overdue })}
                      </span>
                    {:else if isDueToday}
                      <span
                        class="rounded-full border border-border bg-option-orange px-2 py-0.5 text-xs font-medium text-foreground"
                        data-testid="unit-review-due-today"
                      >
                        {m.review_due_today()}
                      </span>
                    {/if}
                  </div>
                  <div class="flex shrink-0 items-center gap-1">
                    <Button
                      variant={expanding ? 'secondary' : 'outline'}
                      size="sm"
                      disabled={busy}
                      class="roomy:min-h-touch"
                      data-testid="unit-review-complete"
                      aria-expanded={expanding}
                      onclick={() => {
                        completingId = expanding ? undefined : review.id;
                      }}
                    >
                      <CheckIcon class="size-3.5" aria-hidden="true" />
                      {m.review_complete()}
                    </Button>
                    <IconButton
                      label={m.review_delete()}
                      disabled={busy}
                      data-testid="unit-review-delete"
                      class="text-muted-foreground hover:text-destructive"
                      onclick={() => void store.deleteReview(review.id)}
                    >
                      <Trash2Icon class="size-4" />
                    </IconButton>
                  </div>
                </div>
                {#if expanding}
                  <ReviewIntervalPicker
                    {intervals}
                    suggested={suggestedNextIntervalDays(review.intervalDays, intervals)}
                    title={m.review_complete_next_prompt()}
                    confirmLabel={m.review_complete_and_schedule()}
                    skipLabel={m.review_complete_only()}
                    {busy}
                    testId="unit-review-complete-picker"
                    onConfirm={(selection: ScheduleUnitReview) =>
                      completeWithNext(review, selection)}
                    onSkip={() => void completeOnly(review)}
                  />
                {/if}
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if completedReviews.length > 0}
        <details class="flex flex-col gap-2" data-testid="unit-reviews-completed">
          <summary class="cursor-pointer text-xs font-medium text-muted-foreground">
            {m.review_panel_completed({ count: completedReviews.length })}
          </summary>
          <ul class="mt-2 flex flex-col gap-1.5">
            {#each completedReviews as review (review.id)}
              {@const busy = store.reviewBusyIds.has(review.id)}
              <li
                class="flex items-center justify-between gap-2 rounded-md border bg-muted/30 px-3 py-2 text-sm text-muted-foreground"
                data-testid="unit-review-item"
                data-review-id={review.id}
                data-review-status="completed"
              >
                <div class="flex flex-wrap items-center gap-2">
                  <span class="tabular-nums">{review.scheduledDate}</span>
                  <span class="rounded-full border px-2 py-0.5 text-xs">
                    {m.review_interval_badge({ days: review.intervalDays })}
                  </span>
                  <span
                    class="rounded-full border bg-option-green px-2 py-0.5 text-xs text-foreground"
                  >
                    {m.review_completed_badge()}
                  </span>
                </div>
                <IconButton
                  label={m.review_delete()}
                  disabled={busy}
                  data-testid="unit-review-delete"
                  class="size-7 text-muted-foreground hover:text-destructive"
                  onclick={() => void store.deleteReview(review.id)}
                >
                  <Trash2Icon class="size-3.5" />
                </IconButton>
              </li>
            {/each}
          </ul>
        </details>
      {/if}
    {/if}
  </section>
{/if}
