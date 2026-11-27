<!--
  Personal "My time" screen (`/me/time`): day/week/month period switcher,
  live-updating summary, kind filter, entry list with target links and
  snapshots, manual entry creation, inline-validated editing, and deletion.
-->
<script lang="ts">
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import ClockIcon from '@lucide/svelte/icons/clock';
  import PlusIcon from '@lucide/svelte/icons/plus';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import type { Regional } from '$lib/logic/regional.ts';
  import {
    formatDayHeading,
    formatPeriodHeading,
    groupEntriesByDay,
    liveSummary,
    periodBounds,
    shiftPeriod,
    TIME_ENTRY_KINDS,
    type TimeEntryKind,
    type TimePeriod,
  } from '$lib/logic/time.ts';
  import { regional as deviceRegional } from '$lib/regional.ts';
  import {
    DeviceToday,
    isSignedOut,
    type CreateTimeEntryRequest,
    type TimeEntry,
    type TimeEntryPatch,
    type TimeEntrySummary,
    type TimeTracker,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import TimeEntryDialog from './TimeEntryDialog.svelte';
  import TimeEntryRow from './TimeEntryRow.svelte';
  import TimeSummaryCard from './TimeSummaryCard.svelte';
  import { KIND_BADGE_CLASS, kindLabel } from './time-labels.ts';

  interface Props {
    tracker: TimeTracker;
    locale: string;
    regional?: Regional | undefined;
    targetHref: (entry: TimeEntry) => string | undefined;
    onOpenTarget: (entry: TimeEntry) => void;
  }

  let { tracker, locale, regional: regionalProp, targetHref, onOpenTarget }: Props = $props();

  const PAGE_SIZE = 50;
  const EMPTY_SUMMARY: TimeEntrySummary = {
    totalMs: 0,
    byKind: { lecture: 0, study: 0, work: 0, other: 0 },
    count: 0,
  };

  const reg = $derived(regionalProp ?? deviceRegional(locale));
  const today = new DeviceToday(() => reg.timeZone);

  let period = $state<TimePeriod>('day');
  let selectedAnchor = $state<CalendarDate | undefined>(undefined);
  const anchor = $derived(selectedAnchor ?? today.date);

  let filterKind = $state<TimeEntryKind | 'all'>('all');
  let entries = $state.raw<TimeEntry[]>([]);
  let summary = $state.raw<TimeEntrySummary>(EMPTY_SUMMARY);
  let status = $state<'loading' | 'ready' | 'failed'>('loading');
  let loadingMore = $state(false);
  let pageIndex = $state(0);

  let dialogOpen = $state(false);
  let dialogMode = $state<'create' | 'edit'>('create');
  let editingEntry = $state.raw<TimeEntry | undefined>(undefined);

  let deleteOpen = $state(false);
  let deletingEntry = $state.raw<TimeEntry | undefined>(undefined);

  const bounds = $derived(periodBounds(anchor, period, reg.timeZone, reg.weekStart));
  const heading = $derived(
    formatPeriodHeading(bounds.firstDay, bounds.lastDay, period, locale, reg.timeZone),
  );
  const displayedSummary = $derived(
    liveSummary(summary, tracker.active, tracker.now, bounds, filterKind),
  );
  const grouped = $derived(groupEntriesByDay(entries, reg.timeZone));
  const hasMore = $derived(entries.length < summary.count);

  let fetchSeq = 0;

  async function loadFirstPage(
    from: number,
    to: number,
    kind: TimeEntryKind | undefined,
  ): Promise<void> {
    const seq = ++fetchSeq;
    status = 'loading';
    try {
      const result = await tracker.listEntries({ from, to, kind, page: 0, perPage: PAGE_SIZE });
      if (seq !== fetchSeq) return;
      entries = result.entries;
      summary = result.summary;
      pageIndex = 0;
      status = 'ready';
    } catch (error) {
      if (seq === fetchSeq && !isSignedOut(error)) status = 'failed';
    }
  }

  $effect(() => {
    const from = bounds.from;
    const to = bounds.to;
    const kind = filterKind === 'all' ? undefined : filterKind;
    // Re-read whenever any time mutation or cross-tab sync increments `tracker.version`.
    const _version = tracker.version;
    void loadFirstPage(from, to, kind);
    return () => {
      fetchSeq += 1;
    };
  });

  async function loadMore(): Promise<void> {
    if (loadingMore || !hasMore) return;
    loadingMore = true;
    const nextPage = pageIndex + 1;
    const seq = fetchSeq;
    try {
      const result = await tracker.listEntries({
        from: bounds.from,
        to: bounds.to,
        kind: filterKind === 'all' ? undefined : filterKind,
        page: nextPage,
        perPage: PAGE_SIZE,
      });
      if (seq !== fetchSeq) return;
      const seen = new Set(entries.map((item) => item.id));
      const added = result.entries.filter((item) => !seen.has(item.id));
      entries = [...entries, ...added];
      summary = result.summary;
      pageIndex = nextPage;
    } catch {
      // Keep already-loaded rows visible; the user can retry Load more.
    } finally {
      loadingMore = false;
    }
  }

  function openCreate(): void {
    dialogMode = 'create';
    editingEntry = undefined;
    dialogOpen = true;
  }

  function openEdit(entry: TimeEntry): void {
    dialogMode = 'edit';
    editingEntry = entry;
    dialogOpen = true;
  }

  function askDelete(entry: TimeEntry): void {
    deletingEntry = entry;
    deleteOpen = true;
  }

  async function handleCreate(request: CreateTimeEntryRequest): Promise<void> {
    await tracker.createEntry(request);
  }

  async function handlePatch(entryId: string, patch: TimeEntryPatch): Promise<void> {
    await tracker.patchEntry(entryId, patch);
  }

  function confirmDelete(): void {
    if (deletingEntry) void tracker.deleteEntry(deletingEntry.id);
  }

  const PERIODS: readonly { id: TimePeriod; label: () => string }[] = [
    { id: 'day', label: () => m.time_period_day() },
    { id: 'week', label: () => m.time_period_week() },
    { id: 'month', label: () => m.time_period_month() },
  ];
</script>

<div class="h-full overflow-y-auto overscroll-contain" data-testid="my-time-screen">
  <div class="mx-auto flex w-full max-w-4xl flex-col gap-5 px-4 py-5 sm:px-6">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 class="text-xl font-semibold tracking-tight sm:text-2xl">{m.time_title()}</h2>
        <p class="mt-0.5 text-sm text-muted-foreground">{m.time_subtitle()}</p>
      </div>
      <Button
        size="sm"
        data-testid="time-add-button"
        class="shrink-0 gap-1.5 roomy:min-h-touch"
        onclick={openCreate}
      >
        <PlusIcon class="size-4" aria-hidden="true" />
        {m.time_add_manual()}
      </Button>
    </div>

    <div class="flex flex-wrap items-center justify-between gap-2">
      <div
        class="inline-flex rounded-lg border bg-muted/50 p-0.5"
        role="group"
        aria-label={m.time_title()}
      >
        {#each PERIODS as item (item.id)}
          <button
            type="button"
            data-testid="time-period-{item.id}"
            aria-pressed={period === item.id}
            class={[
              'min-h-8 rounded-md px-3 text-sm font-medium transition-colors roomy:min-h-touch',
              period === item.id
                ? 'bg-background text-foreground shadow-xs'
                : 'text-muted-foreground hover:text-foreground',
            ]}
            onclick={() => {
              period = item.id;
            }}
          >
            {item.label()}
          </button>
        {/each}
      </div>

      <div class="flex flex-wrap items-center gap-1">
        <IconButton
          label={m.time_period_prev()}
          data-testid="time-period-prev"
          onclick={() => {
            selectedAnchor = shiftPeriod(anchor, period, -1);
          }}
        >
          <ChevronLeftIcon />
        </IconButton>
        <Button
          variant="outline"
          size="sm"
          data-testid="time-period-today"
          class="h-8 px-2.5 text-xs roomy:min-h-touch"
          onclick={() => {
            selectedAnchor = undefined;
          }}
        >
          {m.time_period_today()}
        </Button>
        <IconButton
          label={m.time_period_next()}
          data-testid="time-period-next"
          onclick={() => {
            selectedAnchor = shiftPeriod(anchor, period, 1);
          }}
        >
          <ChevronRightIcon />
        </IconButton>
        <span class="ml-1 text-sm font-medium text-foreground" data-testid="time-period-label">
          {heading}
        </span>
      </div>
    </div>

    <TimeSummaryCard summary={displayedSummary} {filterKind} />

    <div class="flex flex-wrap items-center gap-1.5" role="group" aria-label={m.time_field_kind()}>
      <button
        type="button"
        data-testid="time-filter-all"
        aria-pressed={filterKind === 'all'}
        class={[
          'min-h-8 rounded-full border px-3 text-xs font-medium transition-colors roomy:min-h-touch',
          filterKind === 'all'
            ? 'border-foreground bg-foreground text-background'
            : 'border-border bg-background text-muted-foreground hover:bg-muted',
        ]}
        onclick={() => {
          filterKind = 'all';
        }}
      >
        {m.time_filter_all()}
      </button>
      {#each TIME_ENTRY_KINDS as kind (kind)}
        <button
          type="button"
          data-testid="time-filter-{kind}"
          aria-pressed={filterKind === kind}
          class={[
            'min-h-8 rounded-full border px-3 text-xs font-medium transition-colors roomy:min-h-touch',
            filterKind === kind
              ? [KIND_BADGE_CLASS[kind], 'ring-1 ring-ring/50']
              : 'border-border bg-background text-muted-foreground hover:bg-muted',
          ]}
          onclick={() => {
            filterKind = kind;
          }}
        >
          {kindLabel(kind)}
        </button>
      {/each}
    </div>

    {#if status === 'loading' && entries.length === 0}
      <p class="py-10 text-center text-sm text-muted-foreground" role="status">
        {m.app_loading()}
      </p>
    {:else if status === 'failed'}
      <div class="flex flex-col items-center gap-3 py-10 text-center">
        <p class="text-sm text-muted-foreground" role="alert">{m.app_problem_load()}</p>
        <Button
          variant="outline"
          size="sm"
          class="roomy:min-h-touch"
          onclick={() =>
            void loadFirstPage(
              bounds.from,
              bounds.to,
              filterKind === 'all' ? undefined : filterKind,
            )}
        >
          {m.app_retry()}
        </Button>
      </div>
    {:else if entries.length === 0}
      <div
        data-testid="time-empty-state"
        class="flex flex-col items-center gap-2 rounded-xl border border-dashed py-12 text-center"
      >
        <ClockIcon class="size-8 text-muted-foreground/70" aria-hidden="true" />
        <p class="text-sm font-medium text-foreground">{m.time_summary_empty()}</p>
        <p class="max-w-sm text-xs text-muted-foreground">{m.time_summary_empty_hint()}</p>
      </div>
    {:else}
      <div class="flex flex-col gap-4" data-testid="time-entry-list">
        {#each grouped as group (group.day)}
          <section class="flex flex-col gap-2">
            {#if period !== 'day'}
              <h3 class="text-xs font-semibold tracking-wide text-muted-foreground">
                {formatDayHeading(group.day, locale, reg.timeZone)}
              </h3>
            {/if}
            <ul class="flex flex-col gap-2">
              {#each group.entries as entry (entry.id)}
                {@const href = targetHref(entry)}
                <TimeEntryRow
                  {entry}
                  runningNow={entry.endedAt === null ? tracker.now : null}
                  {locale}
                  timeZone={reg.timeZone}
                  hourCycle={reg.hourCycle}
                  targetHref={href}
                  onOpenTarget={href
                    ? () => {
                        onOpenTarget(entry);
                      }
                    : undefined}
                  onEdit={() => {
                    openEdit(entry);
                  }}
                  onStop={entry.endedAt === null
                    ? () => {
                        void tracker.stopTimer();
                      }
                    : undefined}
                  onDelete={() => {
                    askDelete(entry);
                  }}
                />
              {/each}
            </ul>
          </section>
        {/each}
      </div>

      {#if hasMore}
        <div class="flex justify-center pt-2">
          <Button
            variant="outline"
            size="sm"
            disabled={loadingMore}
            data-testid="time-load-more"
            class="roomy:min-h-touch"
            onclick={() => void loadMore()}
          >
            {m.time_load_more()}
          </Button>
        </div>
      {/if}
    {/if}
  </div>
</div>

<TimeEntryDialog
  bind:open={dialogOpen}
  mode={dialogMode}
  entry={editingEntry}
  timeZone={reg.timeZone}
  onCreate={handleCreate}
  onPatch={handlePatch}
/>

<ConfirmDialog
  bind:open={deleteOpen}
  title={m.time_delete_title()}
  description={m.time_delete_description()}
  confirmLabel={m.shared_delete()}
  destructive
  onConfirm={confirmDelete}
/>
