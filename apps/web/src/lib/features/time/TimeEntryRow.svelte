<!--
  One personal time entry row in the "My time" list.
  Displays kind badge, target title (or snapshot for deleted targets),
  clock range, duration, optional note, and edit/stop/delete actions.
-->
<script lang="ts">
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import SquareIcon from '@lucide/svelte/icons/square';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { HourCycle } from '$lib/logic/format.ts';
  import { formatClockTime, formatTimerClock } from '$lib/logic/time.ts';
  import type { TimeEntry } from '$lib/state/index.ts';

  import {
    entryTargetTitle,
    formatHumanDuration,
    KIND_BADGE_CLASS,
    kindLabel,
  } from './time-labels.ts';

  interface Props {
    entry: TimeEntry;
    /** Current wall clock ms when this row is actively running, else `null`. */
    runningNow: number | null;
    locale: string;
    timeZone: string;
    hourCycle: HourCycle;
    targetHref: string | undefined;
    onOpenTarget: (() => void) | undefined;
    onEdit: () => void;
    onStop: (() => void) | undefined;
    onDelete: () => void;
  }

  let {
    entry,
    runningNow,
    locale,
    timeZone,
    hourCycle,
    targetHref,
    onOpenTarget,
    onEdit,
    onStop,
    onDelete,
  }: Props = $props();

  const running = $derived(entry.endedAt === null);
  const targetTitle = $derived(entryTargetTitle(entry));
  const durationMs = $derived(
    entry.endedAt !== null
      ? Math.max(0, entry.endedAt - entry.startedAt)
      : runningNow !== null
        ? Math.max(0, runningNow - entry.startedAt)
        : 0,
  );
  const startClock = $derived(formatClockTime(entry.startedAt, locale, timeZone, hourCycle));
  const endClock = $derived(
    entry.endedAt !== null
      ? formatClockTime(entry.endedAt, locale, timeZone, hourCycle)
      : m.time_running_now(),
  );

  function handleTargetClick(event: MouseEvent): void {
    if (
      !onOpenTarget ||
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey
    ) {
      return;
    }
    event.preventDefault();
    onOpenTarget();
  }
</script>

<li
  data-testid="time-entry-row"
  data-entry-id={entry.id}
  class={[
    'flex flex-col gap-2 rounded-lg border bg-card px-3 py-2.5 text-card-foreground sm:flex-row sm:items-center sm:gap-3',
    running && 'border-destructive/40 bg-destructive/5',
  ]}
>
  <div class="flex min-w-0 flex-1 flex-col gap-1">
    <div class="flex flex-wrap items-center gap-1.5">
      <span
        class={[
          'inline-flex shrink-0 items-center rounded-full border px-2 py-0.5 text-[0.6875rem] leading-4 font-medium',
          KIND_BADGE_CLASS[entry.kind],
        ]}
        data-testid="time-entry-kind"
      >
        {kindLabel(entry.kind)}
      </span>
      {#if running}
        <span
          class="inline-flex shrink-0 items-center gap-1 rounded-full bg-destructive/15 px-2 py-0.5 text-[0.6875rem] leading-4 font-medium text-destructive"
          data-testid="time-entry-running-badge"
        >
          <span class="size-1.5 rounded-full bg-destructive" aria-hidden="true"></span>
          {m.time_recording()}
        </span>
      {/if}
      {#if targetTitle && targetHref}
        <a
          href={targetHref}
          data-testid="time-entry-target"
          class="inline-flex min-w-0 items-center gap-1 text-sm font-medium text-foreground underline-offset-2 hover:underline"
          onclick={handleTargetClick}
        >
          <span class="truncate">{targetTitle}</span>
          <ExternalLinkIcon class="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
        </a>
      {:else if targetTitle}
        <span
          class="min-w-0 truncate text-sm font-medium text-foreground"
          data-testid="time-entry-target"
        >
          {targetTitle}
        </span>
      {:else}
        <span class="text-sm text-muted-foreground" data-testid="time-entry-target">
          {m.time_target_none()}
        </span>
      {/if}
    </div>

    <div class="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
      <span class="tabular-nums" data-testid="time-entry-range">{startClock} – {endClock}</span>
      {#if entry.note}
        <span aria-hidden="true">·</span>
        <span class="min-w-0 break-words text-foreground/85" data-testid="time-entry-note">
          {entry.note}
        </span>
      {/if}
    </div>
  </div>

  <div class="flex shrink-0 items-center justify-between gap-2 sm:justify-end">
    <span
      class={[
        'text-sm font-semibold tabular-nums',
        running ? 'font-mono text-destructive' : 'text-foreground',
      ]}
      data-testid="time-entry-duration"
    >
      {running ? formatTimerClock(durationMs) : formatHumanDuration(durationMs)}
    </span>

    <div class="flex items-center gap-0.5">
      {#if running && onStop}
        <IconButton
          label={m.time_stop_timer()}
          data-testid="time-entry-stop"
          class="text-destructive hover:text-destructive/80"
          onclick={onStop}
        >
          <SquareIcon class="size-3.5 fill-current" />
        </IconButton>
      {/if}
      <IconButton label={m.time_entry_edit()} data-testid="time-entry-edit" onclick={onEdit}>
        <PencilIcon class="size-4" />
      </IconButton>
      <IconButton
        label={m.time_entry_delete()}
        data-testid="time-entry-delete"
        class="text-muted-foreground hover:text-destructive"
        onclick={onDelete}
      >
        <Trash2Icon class="size-4" />
      </IconButton>
    </div>
  </div>
</li>
