<!--
  Workspace "Today's learning" screen (`/w/<workspace>/learning/today`):
  shows the learner's incomplete overdue units (oldest first, explicitly noting
  no automatic shifting) and incomplete units due today across enrolled live
  courses, with direct unit navigation and one-click completion.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import AlertCircleIcon from '@lucide/svelte/icons/alert-circle';
  import BookOpenIcon from '@lucide/svelte/icons/book-open';
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import CircleIcon from '@lucide/svelte/icons/circle';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { LearningTodayItem, LearningTodayStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { formatAmount } from './course-schedule-labels.ts';

  interface Props {
    store: LearningTodayStore;
    workspaceId: string;
    timeZone: string;
    unitHref: (item: LearningTodayItem) => string;
    onOpenUnit: (item: LearningTodayItem) => void;
    onRefresh: () => void;
  }

  let { store, workspaceId, timeZone, unitHref, onOpenUnit, onRefresh }: Props = $props();

  const uid = $props.id();
  const totalCount = $derived(store.overdueUnits.length + store.todayUnits.length);
  let rootEl = $state<HTMLElement | null>(null);
  let announcement = $state('');

  function openLink(event: MouseEvent, item: LearningTodayItem): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onOpenUnit(item);
  }

  async function completeItem(item: LearningTodayItem): Promise<void> {
    const title = item.title || m.course_unit_untitled();
    const ok = await store.completeUnit(item, workspaceId);
    if (!ok) return;
    announcement = `${title}: ${m.course_unit_completed()}`;
    await tick();
    const nextButton = rootEl?.querySelector<HTMLElement>('[data-testid="learning-item-complete"]');
    (nextButton ?? rootEl)?.focus();
  }
</script>

{#snippet unitRow(item: LearningTodayItem, kind: 'overdue' | 'today')}
  {@const amountText = formatAmount(item.amount, item.amountUnit)}
  {@const unitTitle = item.title || m.course_unit_untitled()}
  {@const courseTitle = item.courseTitle || m.course_untitled()}
  {@const busy = store.isCompleting(item.unitId)}
  <li
    data-testid="learning-{kind}-item"
    data-unit={item.unitId}
    class="flex items-start gap-3 rounded-lg border bg-card px-3 py-2.5 transition-colors hover:bg-muted/40"
  >
    <button
      type="button"
      disabled={busy}
      aria-label={m.learning_today_complete({ title: unitTitle })}
      data-testid="learning-item-complete"
      class="mt-0.5 grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-primary disabled:opacity-50 roomy:size-touch"
      onclick={() => void completeItem(item)}
    >
      <CircleIcon class="size-4.5" aria-hidden="true" />
    </button>

    <a
      href={unitHref(item)}
      class="flex min-w-0 flex-1 flex-col gap-1 outline-none focus-visible:ring-2 focus-visible:ring-ring"
      onclick={(event) => {
        openLink(event, item);
      }}
    >
      <div class="flex flex-wrap items-center gap-1.5">
        <span
          class="inline-flex max-w-full items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-xs font-medium text-muted-foreground"
        >
          <BookOpenIcon class="size-3 shrink-0" aria-hidden="true" />
          <span class="truncate">{courseTitle}</span>
        </span>
        {#if item.myPlannedDate !== null}
          <span
            class="rounded border border-border bg-background px-1.5 py-0.5 text-[0.6875rem] text-muted-foreground"
            data-testid="learning-item-personal"
          >
            {m.course_unit_my_date_personal()}
          </span>
        {/if}
      </div>

      <span class="truncate text-sm font-medium text-foreground" data-testid="learning-item-title">
        {unitTitle}
      </span>

      <div class="flex flex-wrap items-center gap-2 text-xs tabular-nums">
        <span
          class={['font-medium', kind === 'overdue' ? 'text-destructive' : 'text-primary']}
          data-testid="learning-item-date"
        >
          {item.effectiveDate}
        </span>
        {#if amountText !== ''}
          <span
            class="rounded bg-muted px-1.5 py-0.5 text-muted-foreground"
            data-testid="learning-item-amount"
          >
            {amountText}
          </span>
        {/if}
      </div>
    </a>
  </li>
{/snippet}

<div
  bind:this={rootEl}
  tabindex="-1"
  class="h-full overflow-y-auto overscroll-contain outline-none"
  data-testid="learning-today-view"
>
  <p class="sr-only" role="status" aria-live="polite">{announcement}</p>
  <div class="mx-auto flex w-full max-w-4xl flex-col gap-5 px-4 py-5 sm:px-6">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div class="min-w-0">
        <h2 class="text-xl font-semibold tracking-tight sm:text-2xl">
          {m.learning_today_title()}
        </h2>
        <p class="mt-0.5 text-sm text-muted-foreground">{m.learning_today_subtitle()}</p>
        {#if store.today}
          <p class="mt-1 text-xs text-muted-foreground" data-testid="learning-today-date">
            {m.course_schedule_today_label({ today: store.today, timeZone })}
          </p>
        {/if}
      </div>
      <div class="flex items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          class="gap-1.5 roomy:min-h-touch"
          disabled={store.status === 'loading'}
          data-testid="learning-today-refresh"
          onclick={onRefresh}
        >
          <RefreshCwIcon class="size-3.5" aria-hidden="true" />
          {m.learning_today_refresh()}
        </Button>
      </div>
    </div>

    {#if store.status === 'ready'}
      <div class="flex flex-wrap items-center gap-2 text-xs">
        <span
          class={[
            'inline-flex items-center rounded-md border px-2 py-1 font-medium tabular-nums',
            store.overdueUnits.length > 0
              ? 'border-destructive/40 bg-destructive/10 text-destructive'
              : 'border-border bg-muted/60 text-muted-foreground',
          ]}
          data-testid="learning-today-overdue-summary"
        >
          {m.course_schedule_overdue_count({ count: store.overdueUnits.length })}
        </span>
        <span
          class={[
            'inline-flex items-center rounded-md border px-2 py-1 font-medium tabular-nums',
            store.todayUnits.length > 0
              ? 'border-primary/30 bg-primary/10 text-primary'
              : 'border-border bg-muted/60 text-muted-foreground',
          ]}
          data-testid="learning-today-due-summary"
        >
          {m.course_schedule_today_count({ count: store.todayUnits.length })}
        </span>
      </div>
    {/if}

    {#if (store.status === 'idle' || store.status === 'loading') && totalCount === 0}
      <p class="py-10 text-center text-sm text-muted-foreground" role="status">
        {m.app_loading()}
      </p>
    {:else if store.status === 'failed'}
      <div class="flex flex-col items-center gap-3 py-10 text-center">
        <p class="text-sm text-muted-foreground" role="alert">{m.app_problem_load()}</p>
        <Button variant="outline" size="sm" class="roomy:min-h-touch" onclick={onRefresh}>
          {m.app_retry()}
        </Button>
      </div>
    {:else if totalCount === 0}
      <div
        data-testid="learning-today-empty"
        class="flex flex-col items-center gap-2 rounded-xl border border-dashed py-12 text-center"
      >
        <CalendarCheckIcon class="size-8 text-muted-foreground/70" aria-hidden="true" />
        <p class="text-sm font-medium text-foreground">{m.learning_today_empty()}</p>
        <p class="max-w-sm text-xs text-muted-foreground">{m.learning_today_empty_hint()}</p>
      </div>
    {:else}
      {#if store.overdueUnits.length > 0}
        <section
          class="flex flex-col gap-2.5"
          aria-labelledby="{uid}-overdue-heading"
          data-testid="learning-overdue-section"
        >
          <div class="flex flex-col gap-0.5">
            <h3
              id="{uid}-overdue-heading"
              class="flex items-center gap-1.5 text-sm font-semibold text-destructive"
            >
              <AlertCircleIcon class="size-4 shrink-0" aria-hidden="true" />
              {m.learning_today_overdue_heading()} ({store.overdueUnits.length})
            </h3>
            <p class="text-xs text-muted-foreground" data-testid="learning-overdue-hint">
              {m.learning_today_overdue_hint()}
            </p>
          </div>
          <ul class="flex flex-col gap-2">
            {#each store.overdueUnits as item (item.unitId)}
              {@render unitRow(item, 'overdue')}
            {/each}
          </ul>
        </section>
      {/if}

      {#if store.todayUnits.length > 0}
        <section
          class="flex flex-col gap-2.5"
          aria-labelledby="{uid}-today-heading"
          data-testid="learning-today-section"
        >
          <h3
            id="{uid}-today-heading"
            class="flex items-center gap-1.5 text-sm font-semibold text-foreground"
          >
            <CalendarCheckIcon class="size-4 shrink-0 text-primary" aria-hidden="true" />
            {m.learning_today_today_heading()} ({store.todayUnits.length})
          </h3>
          <ul class="flex flex-col gap-2">
            {#each store.todayUnits as item (item.unitId)}
              {@render unitRow(item, 'today')}
            {/each}
          </ul>
        </section>
      {/if}
    {/if}
  </div>
</div>
