<!--
  Reusable spaced-repetition interval picker bar: preset interval chips with the
  suggested interval highlighted, optional custom day/date input, confirm, and
  optional skip/complete-only secondary action.
-->
<script lang="ts">
  import CalendarIcon from '@lucide/svelte/icons/calendar';
  import SparklesIcon from '@lucide/svelte/icons/sparkles';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { parseCalendarDate } from '$lib/logic/dates.ts';
  import {
    MAX_REVIEW_INTERVAL_DAYS,
    MIN_REVIEW_INTERVAL_DAYS,
    type ScheduleUnitReview,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';

  interface Props {
    intervals: readonly number[];
    suggested: number;
    title?: string | undefined;
    confirmLabel: string;
    skipLabel?: string | undefined;
    allowDate?: boolean;
    busy?: boolean;
    testId?: string;
    onConfirm: (selection: ScheduleUnitReview) => void | Promise<void>;
    onSkip?: (() => void) | undefined;
  }

  let {
    intervals,
    suggested,
    title,
    confirmLabel,
    skipLabel,
    allowDate = false,
    busy = false,
    testId = 'review-interval-picker',
    onConfirm,
    onSkip,
  }: Props = $props();

  const uid = $props.id();

  /** Preset choices always include `suggested` so the recommended chip is selectable. */
  const choices = $derived.by(() => {
    const list: number[] = [];
    for (const days of intervals) {
      if (!list.includes(days)) list.push(days);
    }
    if (
      Number.isInteger(suggested) &&
      suggested >= MIN_REVIEW_INTERVAL_DAYS &&
      suggested <= MAX_REVIEW_INTERVAL_DAYS &&
      !list.includes(suggested)
    ) {
      list.push(suggested);
    }
    return list.sort((a, b) => a - b);
  });

  let picked = $state<number | 'custom' | undefined>(undefined);
  let customDays = $state('');
  let customDate = $state('');
  let validationError = $state('');
  let validationField = $state<'days' | 'date' | null>(null);

  const selected = $derived(picked ?? suggested);
  const isCustom = $derived(selected === 'custom');

  function clearError(): void {
    validationError = '';
    validationField = null;
  }

  function selectPreset(days: number): void {
    picked = days;
    clearError();
  }

  function selectCustom(): void {
    picked = 'custom';
    if (!allowDate && customDays === '') customDays = String(suggested);
    clearError();
  }

  function buildRequest(): ScheduleUnitReview | null {
    if (!isCustom) {
      return { intervalDays: selected as number };
    }
    const trimmedDate = customDate.trim();
    const trimmedDays = customDays.trim();
    if (allowDate && trimmedDate !== '') {
      if (!parseCalendarDate(trimmedDate)) {
        validationError = m.review_date_invalid();
        validationField = 'date';
        return null;
      }
      if (trimmedDays === '') {
        return { scheduledDate: trimmedDate };
      }
    }
    const isDigits = /^\d+$/.test(trimmedDays);
    const parsed = isDigits ? Number(trimmedDays) : NaN;
    if (
      !isDigits ||
      !Number.isInteger(parsed) ||
      parsed < MIN_REVIEW_INTERVAL_DAYS ||
      parsed > MAX_REVIEW_INTERVAL_DAYS
    ) {
      validationError = m.review_interval_invalid({
        min: MIN_REVIEW_INTERVAL_DAYS,
        max: MAX_REVIEW_INTERVAL_DAYS,
      });
      validationField = 'days';
      return null;
    }
    return allowDate && trimmedDate !== ''
      ? { intervalDays: parsed, scheduledDate: trimmedDate }
      : { intervalDays: parsed };
  }

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    if (busy) return;
    clearError();
    const request = buildRequest();
    if (!request) return;
    void onConfirm(request);
  }
</script>

<form
  novalidate
  class="flex w-full flex-col gap-2.5 rounded-lg border bg-muted/40 p-3"
  data-testid={testId}
  onsubmit={submit}
>
  {#if title}
    <div class="flex items-center gap-1.5 text-sm font-medium text-foreground">
      <CalendarIcon class="size-4 shrink-0 text-primary" aria-hidden="true" />
      <span>{title}</span>
    </div>
  {/if}

  <div class="flex flex-wrap items-center gap-1.5" role="group" aria-label={title ?? confirmLabel}>
    {#each choices as days (days)}
      {@const active = selected === days}
      {@const isRecommended = days === suggested}
      <button
        type="button"
        disabled={busy}
        aria-pressed={active}
        data-testid="{testId}-option-{days}"
        data-suggested={isRecommended ? 'true' : undefined}
        class={[
          'inline-flex min-h-8 items-center gap-1 rounded-md border px-2.5 text-xs font-medium transition-colors roomy:min-h-touch',
          active
            ? 'border-primary bg-primary text-primary-foreground'
            : isRecommended
              ? 'border-primary/60 bg-background text-foreground hover:bg-accent'
              : 'border-border bg-background text-muted-foreground hover:bg-accent hover:text-foreground',
        ]}
        onclick={() => {
          selectPreset(days);
        }}
      >
        {#if isRecommended}
          <SparklesIcon class="size-3 shrink-0" aria-hidden="true" />
        {/if}
        <span>{m.review_interval_days({ days })}</span>
        {#if isRecommended}
          <span class="sr-only"> ({m.review_suggest_recommended()})</span>
        {/if}
      </button>
    {/each}
    <button
      type="button"
      disabled={busy}
      aria-pressed={isCustom}
      data-testid="{testId}-custom"
      class={[
        'inline-flex min-h-8 items-center rounded-md border px-2.5 text-xs font-medium transition-colors roomy:min-h-touch',
        isCustom
          ? 'border-primary bg-primary text-primary-foreground'
          : 'border-border bg-background text-muted-foreground hover:bg-accent hover:text-foreground',
      ]}
      onclick={selectCustom}
    >
      {m.review_interval_custom()}
    </button>
  </div>

  {#if isCustom}
    <div class="flex flex-wrap items-end gap-2">
      <label class="flex flex-col gap-1 text-xs text-muted-foreground" for="{uid}-days">
        <span>{m.review_interval_custom_label()}</span>
        <Input
          id="{uid}-days"
          type="text"
          inputmode="numeric"
          autocomplete="off"
          bind:value={customDays}
          disabled={busy}
          placeholder={m.review_interval_custom_placeholder()}
          aria-invalid={validationField === 'days' ? 'true' : undefined}
          aria-describedby={validationField === 'days' ? `${uid}-error` : undefined}
          data-testid="{testId}-custom-days"
          class="h-8 w-36 bg-background text-sm roomy:h-touch"
          oninput={() => {
            if (allowDate && customDays.trim() !== '') customDate = '';
            if (validationError !== '') clearError();
          }}
        />
      </label>
      {#if allowDate}
        <label class="flex flex-col gap-1 text-xs text-muted-foreground" for="{uid}-date">
          <span>{m.review_interval_custom_date_label()}</span>
          <Input
            id="{uid}-date"
            type="date"
            bind:value={customDate}
            disabled={busy}
            aria-invalid={validationField === 'date' ? 'true' : undefined}
            aria-describedby={validationField === 'date' ? `${uid}-error` : undefined}
            data-testid="{testId}-custom-date"
            class="h-8 w-40 bg-background text-sm roomy:h-touch"
            oninput={() => {
              if (customDate.trim() !== '') customDays = '';
              if (validationError !== '') clearError();
            }}
          />
        </label>
      {/if}
    </div>
  {/if}

  {#if validationError}
    <p id="{uid}-error" class="text-xs text-destructive" role="alert" data-testid="{testId}-error">
      {validationError}
    </p>
  {/if}

  <div class="flex flex-wrap items-center gap-2">
    <Button
      type="submit"
      size="sm"
      disabled={busy}
      class="roomy:min-h-touch"
      data-testid="{testId}-confirm"
    >
      {confirmLabel}
    </Button>
    {#if skipLabel && onSkip}
      <Button
        type="button"
        variant="ghost"
        size="sm"
        disabled={busy}
        class="text-muted-foreground roomy:min-h-touch"
        data-testid="{testId}-skip"
        onclick={onSkip}
      >
        {skipLabel}
      </Button>
    {/if}
  </div>
</form>
