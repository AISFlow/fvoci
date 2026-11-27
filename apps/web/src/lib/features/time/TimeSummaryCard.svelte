<!--
  Summary panel for the selected day/week/month range: total recorded time
  (including live elapsed delta when a running timer falls in the period),
  proportional stacked bar, and per-kind duration breakdown.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { TIME_ENTRY_KINDS, type TimeEntryKind } from '$lib/logic/time.ts';
  import type { TimeEntrySummary } from '$lib/state/index.ts';

  import {
    formatHumanDuration,
    KIND_BADGE_CLASS,
    KIND_BAR_CLASS,
    kindLabel,
  } from './time-labels.ts';

  interface Props {
    summary: TimeEntrySummary;
    filterKind: TimeEntryKind | 'all';
  }

  let { summary, filterKind }: Props = $props();

  const segments = $derived(
    summary.totalMs > 0
      ? TIME_ENTRY_KINDS.filter((kind) => summary.byKind[kind] > 0).map((kind) => ({
          kind,
          ms: summary.byKind[kind],
          pct: (summary.byKind[kind] / summary.totalMs) * 100,
        }))
      : [],
  );
</script>

<section
  aria-label={m.time_summary_total()}
  data-testid="time-summary-card"
  class="flex flex-col gap-3 rounded-xl border bg-card p-4 text-card-foreground shadow-xs"
>
  <div class="flex flex-wrap items-baseline justify-between gap-2">
    <div>
      <p class="text-xs font-medium text-muted-foreground">{m.time_summary_total()}</p>
      <p
        class="mt-0.5 text-2xl font-semibold tracking-tight tabular-nums sm:text-3xl"
        data-testid="time-summary-total"
      >
        {formatHumanDuration(summary.totalMs)}
      </p>
    </div>
    <span class="text-xs text-muted-foreground tabular-nums" data-testid="time-summary-count">
      {m.time_summary_entries({ count: summary.count })}
    </span>
  </div>

  <div
    class="flex h-2.5 w-full overflow-hidden rounded-full bg-muted"
    role="img"
    aria-label={m.time_summary_total()}
  >
    {#each segments as seg (seg.kind)}
      <div
        class={['h-full transition-[width] duration-300', KIND_BAR_CLASS[seg.kind]]}
        style:width="{seg.pct}%"
        title={`${kindLabel(seg.kind)}: ${formatHumanDuration(seg.ms)}`}
      ></div>
    {/each}
  </div>

  <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
    {#each TIME_ENTRY_KINDS as kind (kind)}
      {@const ms = summary.byKind[kind]}
      <div
        data-testid="time-summary-kind-{kind}"
        class={[
          'flex items-center justify-between gap-2 rounded-lg border px-2.5 py-2 text-xs',
          filterKind === kind ? KIND_BADGE_CLASS[kind] : 'border-border bg-background/60',
        ]}
      >
        <span class="flex min-w-0 items-center gap-1.5">
          <span class={['size-2 shrink-0 rounded-full', KIND_BAR_CLASS[kind]]} aria-hidden="true"
          ></span>
          <span class="truncate font-medium">{kindLabel(kind)}</span>
        </span>
        <span class="shrink-0 font-semibold tabular-nums">{formatHumanDuration(ms)}</span>
      </div>
    {/each}
  </div>
</section>
