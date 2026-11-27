<!--
  Workspace "Today's reviews" screen (`GET /w/{workspaceId}/learning/reviews/today`):
  lists the caller's overdue and due-today pending reviews across enrolled courses
  in the active workspace, lets them jump directly to any unit, or complete a
  review inline with an optional follow-up interval.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import CheckIcon from '@lucide/svelte/icons/check';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    suggestedNextIntervalDays,
    type ScheduleUnitReview,
    type TodayReviewItem,
    type TodayReviewsStore,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import ReviewIntervalPicker from './ReviewIntervalPicker.svelte';

  interface Props {
    store: TodayReviewsStore;
    workspaceId: string;
    autoLoad?: boolean;
    unitHref: (item: TodayReviewItem) => string;
    onOpenUnit: (item: TodayReviewItem) => void;
  }

  let { store, workspaceId, autoLoad = true, unitHref, onOpenUnit }: Props = $props();

  let completing = $state<{ workspaceId: string; id: string } | undefined>(undefined);
  let feedbackState = $state<{ workspaceId: string; text: string } | undefined>(undefined);

  const completingId = $derived(
    completing?.workspaceId === workspaceId ? completing.id : undefined,
  );
  const feedback = $derived(feedbackState?.workspaceId === workspaceId ? feedbackState.text : '');
  const overdueCount = $derived(store.items.filter((item) => item.overdueDays > 0).length);

  $effect(() => {
    if (!autoLoad) return;
    const ws = workspaceId;
    const _version = store.version;
    untrack(() => {
      void store.load(ws);
    });
  });

  function openLink(event: MouseEvent, item: TodayReviewItem): void {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) {
      return;
    }
    event.preventDefault();
    onOpenUnit(item);
  }

  async function completeWithNext(
    item: TodayReviewItem,
    selection: ScheduleUnitReview,
  ): Promise<void> {
    const targetWs = workspaceId;
    const nextDays = selection.intervalDays;
    const result = await store.complete(
      item,
      nextDays !== undefined ? { scheduleNextIntervalDays: nextDays } : {},
    );
    if (!result) return;
    if (completing?.workspaceId === targetWs && completing.id === item.id) completing = undefined;
    feedbackState = {
      workspaceId: targetWs,
      text: result.nextReview
        ? m.review_completed_next_status({ date: result.nextReview.scheduledDate })
        : m.review_completed_status(),
    };
  }

  async function completeOnly(item: TodayReviewItem): Promise<void> {
    const targetWs = workspaceId;
    const result = await store.complete(item, {});
    if (!result) return;
    if (completing?.workspaceId === targetWs && completing.id === item.id) completing = undefined;
    feedbackState = { workspaceId: targetWs, text: m.review_completed_status() };
  }
</script>

<div class="h-full overflow-y-auto overscroll-contain" data-testid="today-reviews-screen">
  <div class="mx-auto flex w-full max-w-4xl flex-col gap-5 px-4 py-5 sm:px-6">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 class="text-xl font-semibold tracking-tight sm:text-2xl">
          {m.today_reviews_title()}
        </h2>
        <p class="mt-0.5 text-sm text-muted-foreground">{m.today_reviews_subtitle()}</p>
      </div>
      <div class="flex items-center gap-2">
        {#if store.status === 'ready' && store.items.length > 0}
          <span
            class="rounded-full border bg-muted/50 px-2.5 py-1 text-xs font-medium text-foreground tabular-nums"
            data-testid="today-reviews-summary"
          >
            {m.today_reviews_summary({ total: store.items.length, overdue: overdueCount })}
          </span>
        {/if}
        <Button
          variant="outline"
          size="sm"
          class="gap-1.5 roomy:min-h-touch"
          disabled={store.status === 'loading'}
          data-testid="today-reviews-refresh"
          onclick={() => void store.load(workspaceId)}
        >
          <RefreshCwIcon class="size-3.5" aria-hidden="true" />
          {m.today_reviews_refresh()}
        </Button>
      </div>
    </div>

    {#if feedback}
      <div
        class="rounded-lg border bg-muted/50 px-3 py-2 text-sm text-foreground"
        role="status"
        data-testid="today-reviews-status"
      >
        {feedback}
      </div>
    {/if}

    {#if (store.status === 'idle' || store.status === 'loading') && store.items.length === 0}
      <p class="py-10 text-center text-sm text-muted-foreground" role="status">
        {m.app_loading()}
      </p>
    {:else if store.status === 'failed'}
      <div class="flex flex-col items-center gap-3 py-10 text-center" role="alert">
        <p class="text-sm text-muted-foreground">{m.app_problem_load()}</p>
        <Button
          variant="outline"
          size="sm"
          class="roomy:min-h-touch"
          onclick={() => void store.load(workspaceId)}
        >
          {m.app_retry()}
        </Button>
      </div>
    {:else if store.items.length === 0}
      <div
        class="flex flex-col items-center gap-2 rounded-xl border border-dashed py-12 text-center"
        data-testid="today-reviews-empty"
      >
        <CalendarCheckIcon class="size-8 text-muted-foreground/70" aria-hidden="true" />
        <p class="text-sm font-medium text-foreground">{m.today_reviews_empty()}</p>
        <p class="max-w-sm text-xs text-muted-foreground">{m.today_reviews_empty_hint()}</p>
      </div>
    {:else}
      <ul class="flex flex-col gap-3" data-testid="today-reviews-list">
        {#each store.items as item (item.id)}
          {@const busy = store.busyIds.has(item.id)}
          {@const expanding = completingId === item.id}
          <li
            class="flex flex-col gap-3 rounded-xl border bg-background p-4"
            data-testid="today-review-item"
            data-review-id={item.id}
            data-overdue={item.overdueDays > 0 ? 'true' : 'false'}
          >
            <div class="flex flex-wrap items-start justify-between gap-3">
              <div class="flex min-w-0 flex-1 flex-col gap-1">
                <div class="flex flex-wrap items-center gap-2">
                  {#if item.overdueDays > 0}
                    <span
                      class="rounded-full border border-destructive/30 bg-destructive/10 px-2 py-0.5 text-xs font-medium text-destructive"
                      data-testid="today-review-overdue"
                    >
                      {m.review_overdue_days({ days: item.overdueDays })}
                    </span>
                  {:else}
                    <span
                      class="rounded-full border border-border bg-option-orange px-2 py-0.5 text-xs font-medium text-foreground"
                      data-testid="today-review-due-today"
                    >
                      {m.review_due_today()}
                    </span>
                  {/if}
                  <span class="truncate text-xs text-muted-foreground">
                    {item.courseTitle || m.course_untitled()}
                  </span>
                </div>
                <a
                  href={unitHref(item)}
                  class="truncate text-base font-semibold text-foreground hover:underline focus-visible:underline focus-visible:outline-none"
                  data-testid="today-review-open"
                  onclick={(event) => {
                    openLink(event, item);
                  }}
                >
                  {item.unitTitle || m.course_unit_untitled()}
                </a>
                <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                  <span class="tabular-nums">{item.scheduledDate}</span>
                  <span aria-hidden="true">·</span>
                  <span>{m.review_interval_badge({ days: item.intervalDays })}</span>
                </div>
              </div>

              <div class="flex shrink-0 flex-wrap items-center gap-2">
                <Button
                  variant={expanding ? 'secondary' : 'default'}
                  size="sm"
                  disabled={busy}
                  class="roomy:min-h-touch"
                  data-testid="today-review-complete"
                  aria-expanded={expanding}
                  onclick={() => {
                    completing = expanding ? undefined : { workspaceId, id: item.id };
                  }}
                >
                  <CheckIcon class="size-3.5" aria-hidden="true" />
                  {m.review_complete()}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  class="roomy:min-h-touch"
                  data-testid="today-review-go"
                  onclick={() => {
                    onOpenUnit(item);
                  }}
                >
                  {m.today_reviews_open_unit()}
                  <ArrowRightIcon class="size-3.5" aria-hidden="true" />
                </Button>
              </div>
            </div>

            {#if expanding}
              {@const itemIntervals = store.intervalsFor(item.containerId)}
              <ReviewIntervalPicker
                intervals={itemIntervals}
                suggested={suggestedNextIntervalDays(item.intervalDays, itemIntervals)}
                title={m.review_complete_next_prompt()}
                confirmLabel={m.review_complete_and_schedule()}
                skipLabel={m.review_complete_only()}
                {busy}
                testId="today-review-complete-picker"
                onConfirm={(selection: ScheduleUnitReview) => completeWithNext(item, selection)}
                onSkip={() => void completeOnly(item)}
              />
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>
