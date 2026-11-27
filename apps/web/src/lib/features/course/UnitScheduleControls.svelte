<!--
  A unit's study amount (`amount` + `minutes`/`pages`/`problems`), shared
  scheduled date (`scheduledDate`), and personal planned date (`myPlannedDate`)
  in the unit header: one line of what is set, which opens their controls, or
  nothing until the unit's menu opens them (`openings.schedule`). Only for
  `kind === 'unit'` (never chapters), and never mounted on public share links.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { Attachment } from 'svelte/attachments';
  import { on } from 'svelte/events';

  import CalendarIcon from '@lucide/svelte/icons/calendar';
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import GaugeIcon from '@lucide/svelte/icons/gauge';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isComposing } from '$lib/keys.ts';
  import { isValidUnitAmount } from '$lib/logic/course-schedule.ts';
  import {
    AMOUNT_UNITS,
    UNIT_AMOUNT_MAX,
    UNIT_AMOUNT_MIN,
    type AmountUnit,
    type CourseStore,
    type CourseUnit,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';

  import { amountUnitLabel, formatAmount } from './course-schedule-labels.ts';
  import type { UnitOpenings } from './unit-openings.svelte.ts';

  interface Props {
    course: CourseStore;
    unit: CourseUnit;
    openings: UnitOpenings;
  }

  let { course, unit, openings }: Props = $props();

  const uid = $props.id();
  const scheduled = $derived(course.schedule.unit(unit.id));
  const amount = $derived(scheduled ? scheduled.amount : (unit.amount ?? null));
  const amountUnit = $derived<AmountUnit | null>(
    scheduled ? scheduled.amountUnit : (unit.amountUnit ?? null),
  );
  const sharedDate = $derived(scheduled ? scheduled.scheduledDate : (unit.scheduledDate ?? null));
  const myPlannedDate = $derived(scheduled ? scheduled.myPlannedDate : null);
  const effectiveDate = $derived(
    scheduled ? scheduled.effectiveDate : (myPlannedDate ?? sharedDate),
  );
  const overdue = $derived(scheduled?.overdue ?? false);
  const isToday = $derived(
    !scheduled?.completed &&
      effectiveDate !== null &&
      course.schedule.today !== '' &&
      effectiveDate === course.schedule.today,
  );
  const busy = $derived(course.schedule.isUnitBusy(unit.id));
  const canSet = $derived(course.canEdit || course.canTrackProgress);
  const setting = $derived(canSet && openings.schedule === unit.id);
  const anySet = $derived(amount !== null || sharedDate !== null || myPlannedDate !== null);

  let editingAmountFor = $state<string | undefined>(undefined);
  let draftAmount = $state('');
  let draftUnit = $state<AmountUnit>('minutes');
  let amountError = $state('');
  let dateErrorFor = $state<{ unitId: string; text: string } | undefined>(undefined);
  let amountInput = $state<HTMLInputElement | null>(null);
  let amountTrigger = $state<HTMLElement | null>(null);
  let sharedDateInput = $state<HTMLInputElement | null>(null);
  let myDateInput = $state<HTMLInputElement | null>(null);
  let summary = $state<HTMLButtonElement | null>(null);
  let controls = $state<HTMLElement | null>(null);

  const editingAmount = $derived(editingAmountFor === unit.id);
  const dateError = $derived(dateErrorFor?.unitId === unit.id ? dateErrorFor.text : '');

  async function openAmountEditor(): Promise<void> {
    if (!course.canEdit || busy) return;
    const targetId = unit.id;
    draftAmount = amount === null ? '' : String(amount);
    draftUnit = amountUnit ?? 'minutes';
    amountError = '';
    editingAmountFor = targetId;
    await tick();
    if (editingAmountFor === targetId) {
      amountInput?.focus();
      amountInput?.select();
    }
  }

  async function closeAmountEditor(restoreFocus = true): Promise<void> {
    const targetId = unit.id;
    editingAmountFor = undefined;
    amountError = '';
    if (!restoreFocus) return;
    await tick();
    if (unit.id === targetId) amountTrigger?.focus();
  }

  async function saveAmount(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!course.canEdit || busy) return;
    const targetId = unit.id;
    const trimmed = draftAmount.trim();
    if (trimmed === '') {
      const res = await course.schedule.patchUnitMeta(targetId, { amount: null, amountUnit: null });
      if (res.ok && editingAmountFor === targetId) await closeAmountEditor();
      return;
    }
    const parsed = Number(trimmed);
    if (!/^\d+$/.test(trimmed) || !isValidUnitAmount(parsed)) {
      amountError = m.course_schedule_error_amount({ min: UNIT_AMOUNT_MIN, max: UNIT_AMOUNT_MAX });
      return;
    }
    amountError = '';
    const res = await course.schedule.patchUnitMeta(targetId, {
      amount: parsed,
      amountUnit: draftUnit,
    });
    if (res.ok && editingAmountFor === targetId) await closeAmountEditor();
    else if (res.error && editingAmountFor === targetId) {
      amountError = m.course_schedule_error_amount({ min: UNIT_AMOUNT_MIN, max: UNIT_AMOUNT_MAX });
    }
  }

  async function clearAmount(): Promise<void> {
    if (!course.canEdit || busy) return;
    const targetId = unit.id;
    const res = await course.schedule.patchUnitMeta(targetId, { amount: null, amountUnit: null });
    if (res.ok && unit.id === targetId) {
      await tick();
      amountTrigger?.focus();
    }
  }

  async function changeSharedDate(next: string): Promise<void> {
    if (!course.canEdit || busy) return;
    const targetId = unit.id;
    const value = next.trim();
    dateErrorFor = undefined;
    const res = await course.schedule.patchUnitMeta(targetId, {
      scheduledDate: value === '' ? null : value,
    });
    if (!res.ok && unit.id === targetId) {
      if (sharedDateInput) sharedDateInput.value = sharedDate ?? '';
      if (res.error) dateErrorFor = { unitId: targetId, text: m.course_schedule_error_date() };
    }
  }

  async function changeMyDate(next: string): Promise<void> {
    if (!course.canTrackProgress || busy) return;
    const targetId = unit.id;
    const value = next.trim();
    dateErrorFor = undefined;
    const res = await course.schedule.setUnitPlannedDate(targetId, value === '' ? null : value);
    if (!res.ok && unit.id === targetId) {
      if (myDateInput) myDateInput.value = myPlannedDate ?? '';
      if (res.error) dateErrorFor = { unitId: targetId, text: m.course_schedule_error_date() };
    }
  }

  /** The controls, just opened: the caret in the first of them, and Escape closes them. */
  const opened: Attachment<HTMLElement> = (node) => {
    node.querySelector<HTMLElement>('button, input, select')?.focus();
    return on(node, 'keydown', onSettingKeydown);
  };

  /**
   * Closes the controls, the focus on the summary of what was set; where nothing was (the
   * summary goes with the controls), on the unit's menu, which opened them.
   */
  async function closeSetting(): Promise<void> {
    const menu = controls?.closest('header')?.querySelector<HTMLElement>('[data-unit-menu]');
    editingAmountFor = undefined;
    amountError = '';
    if (openings.schedule === unit.id) openings.schedule = undefined;
    await tick();
    (summary ?? menu)?.focus();
  }

  function onSettingKeydown(event: KeyboardEvent): void {
    if (event.key !== 'Escape' || event.defaultPrevented || isComposing(event)) return;
    event.preventDefault();
    void closeSetting();
  }

  function onAmountKeydown(event: KeyboardEvent): void {
    if (isComposing(event)) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      void closeAmountEditor();
    }
  }
</script>

{#if unit.kind === 'unit' && (setting || anySet)}
  <div
    bind:this={controls}
    class="flex w-full flex-wrap items-center gap-2 text-xs"
    data-testid="unit-schedule-controls"
  >
    {#if setting}
      <div
        class="flex w-full flex-wrap items-center gap-2"
        role="group"
        aria-label={m.course_unit_set_schedule()}
        data-testid="unit-schedule-setting"
        {@attach opened}
      >
        {#if editingAmount}
          <form
            class="flex flex-wrap items-center gap-1.5"
            onsubmit={(event) => void saveAmount(event)}
            data-testid="unit-amount-form"
          >
            <Input
              bind:ref={amountInput}
              type="text"
              inputmode="numeric"
              bind:value={draftAmount}
              aria-label={m.course_amount_label()}
              aria-invalid={amountError !== '' ? 'true' : undefined}
              aria-describedby={amountError !== '' ? `${uid}-amount-err` : undefined}
              disabled={busy}
              data-testid="unit-amount-input"
              class="h-7 w-24 roomy:h-touch"
              onkeydown={onAmountKeydown}
            />
            <select
              bind:value={draftUnit}
              aria-label={m.course_amount_unit_label()}
              disabled={busy}
              data-testid="unit-amount-unit-select"
              class="h-7 rounded-md border border-input-border bg-background px-2 text-base text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring md:text-sm roomy:h-touch"
            >
              {#each AMOUNT_UNITS as option (option)}
                <option value={option}>{amountUnitLabel(option)}</option>
              {/each}
            </select>
            <Button
              type="submit"
              size="sm"
              variant="secondary"
              disabled={busy}
              data-testid="unit-amount-save"
              class="h-7 px-2 text-xs roomy:min-h-touch"
            >
              {m.course_last_position_save()}
            </Button>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              disabled={busy}
              data-testid="unit-amount-cancel"
              class="h-7 px-2 text-xs roomy:min-h-touch"
              onclick={() => void closeAmountEditor()}
            >
              {m.shared_cancel()}
            </Button>
            {#if amountError}
              <p
                id="{uid}-amount-err"
                class="w-full text-xs text-destructive"
                role="alert"
                data-testid="unit-amount-error"
              >
                {amountError}
              </p>
            {/if}
          </form>
        {:else if amount !== null}
          <div
            class="inline-flex items-center gap-1 rounded-md border bg-muted/50 py-0.5 pr-1 pl-2 text-muted-foreground"
            data-testid="unit-amount-badge"
          >
            {#if course.canEdit}
              <button
                bind:this={amountTrigger}
                type="button"
                class="flex min-h-6 items-center gap-1 hover:text-foreground roomy:min-h-touch"
                disabled={busy}
                data-testid="unit-amount-edit"
                onclick={() => void openAmountEditor()}
              >
                <GaugeIcon class="size-3.5 shrink-0" aria-hidden="true" />
                <span class="sr-only">{m.course_amount_edit()}: </span>
                <span data-testid="unit-amount-value">{formatAmount(amount, amountUnit)}</span>
              </button>
              <IconButton
                label={m.course_amount_clear()}
                disabled={busy}
                data-testid="unit-amount-clear"
                class="size-6 shrink-0 text-muted-foreground hover:text-foreground roomy:size-touch"
                onclick={() => void clearAmount()}
              >
                <XIcon class="size-3" />
              </IconButton>
            {:else}
              <GaugeIcon class="size-3.5 shrink-0" aria-hidden="true" />
              <span class="pr-1" data-testid="unit-amount-value"
                >{formatAmount(amount, amountUnit)}</span
              >
            {/if}
          </div>
        {:else if course.canEdit}
          <Button
            bind:ref={amountTrigger}
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-xs text-muted-foreground roomy:min-h-touch"
            disabled={busy}
            data-testid="unit-amount-add"
            onclick={() => void openAmountEditor()}
          >
            <GaugeIcon class="size-3.5" aria-hidden="true" />
            {m.course_amount_add()}
          </Button>
        {/if}

        {#if course.canEdit}
          <div class="inline-flex items-center gap-1.5 text-muted-foreground">
            <label for="{uid}-shared-date" class="inline-flex items-center gap-1">
              <CalendarIcon class="size-3.5 shrink-0" aria-hidden="true" />
              <span>{m.course_unit_shared_date()}</span>
            </label>
            <input
              bind:this={sharedDateInput}
              id="{uid}-shared-date"
              type="date"
              value={sharedDate ?? ''}
              disabled={busy}
              data-testid="unit-shared-date-input"
              class="h-7 rounded-md border border-input-border bg-background px-2 text-base text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring md:text-sm roomy:h-touch"
              onchange={(event) => void changeSharedDate(event.currentTarget.value)}
            />
            {#if sharedDate}
              <IconButton
                label={m.course_unit_shared_date_clear()}
                disabled={busy}
                data-testid="unit-shared-date-clear"
                class="size-6 shrink-0 text-muted-foreground hover:text-foreground roomy:size-touch"
                onclick={() => void changeSharedDate('')}
              >
                <XIcon class="size-3.5" />
              </IconButton>
            {/if}
          </div>
        {:else if sharedDate}
          <span
            class="inline-flex items-center gap-1 rounded-md border bg-muted/50 px-2 py-1 text-muted-foreground"
            data-testid="unit-shared-date-text"
          >
            <CalendarIcon class="size-3.5 shrink-0" aria-hidden="true" />
            {m.course_unit_shared_date()}: {sharedDate}
          </span>
        {/if}

        {#if course.canTrackProgress}
          <div class="inline-flex items-center gap-1.5 text-muted-foreground">
            <label for="{uid}-my-date" class="inline-flex items-center gap-1">
              <CalendarCheckIcon class="size-3.5 shrink-0" aria-hidden="true" />
              <span>{m.course_unit_my_date()}</span>
            </label>
            <input
              bind:this={myDateInput}
              id="{uid}-my-date"
              type="date"
              value={myPlannedDate ?? ''}
              disabled={busy}
              data-testid="unit-my-date-input"
              class="h-7 rounded-md border border-input-border bg-background px-2 text-base text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring md:text-sm roomy:h-touch"
              onchange={(event) => void changeMyDate(event.currentTarget.value)}
            />
            {#if myPlannedDate}
              <IconButton
                label={m.course_unit_my_date_clear()}
                disabled={busy}
                data-testid="unit-my-date-clear"
                class="size-6 shrink-0 text-muted-foreground hover:text-foreground roomy:size-touch"
                onclick={() => void changeMyDate('')}
              >
                <XIcon class="size-3.5" />
              </IconButton>
            {/if}
          </div>
        {/if}
        <Button
          type="button"
          size="sm"
          variant="secondary"
          class="h-7 px-2 text-xs roomy:min-h-touch"
          data-testid="unit-schedule-done"
          onclick={() => void closeSetting()}
        >
          {m.ui_done()}
        </Button>
      </div>
    {:else}
      {#snippet setParts()}
        {#if amount !== null}
          <span class="inline-flex items-center gap-1">
            <GaugeIcon class="size-3.5 shrink-0" aria-hidden="true" />
            <span data-testid="unit-amount-value">{formatAmount(amount, amountUnit)}</span>
          </span>
        {/if}
        {#if sharedDate}
          <span class="inline-flex items-center gap-1" data-testid="unit-shared-date-text">
            <CalendarIcon class="size-3.5 shrink-0" aria-hidden="true" />
            {m.course_unit_shared_date()}: {sharedDate}
          </span>
        {/if}
        {#if myPlannedDate}
          <span class="inline-flex items-center gap-1" data-testid="unit-my-date-text">
            <CalendarCheckIcon class="size-3.5 shrink-0" aria-hidden="true" />
            {m.course_unit_my_date()}: {myPlannedDate}
          </span>
        {/if}
      {/snippet}
      {#if canSet}
        <button
          bind:this={summary}
          type="button"
          class="inline-flex min-h-7 flex-wrap items-center gap-x-3 gap-y-1 rounded-md px-1.5 text-muted-foreground hover:bg-muted hover:text-foreground roomy:min-h-touch"
          data-testid="unit-schedule-summary"
          onclick={() => {
            openings.schedule = unit.id;
          }}
        >
          <span class="sr-only">{m.course_unit_schedule_summary()}: </span>
          {@render setParts()}
        </button>
      {:else}
        <span
          class="inline-flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground"
          data-testid="unit-schedule-summary"
        >
          {@render setParts()}
        </span>
      {/if}
    {/if}

    {#if overdue}
      <span
        class="inline-flex items-center rounded-md border border-destructive/40 bg-destructive/10 px-1.5 py-0.5 font-medium text-destructive"
        data-testid="unit-status-overdue"
      >
        {m.course_unit_badge_overdue()}
      </span>
    {:else if isToday}
      <span
        class="inline-flex items-center rounded-md border border-primary/30 bg-primary/10 px-1.5 py-0.5 font-medium text-primary"
        data-testid="unit-status-today"
      >
        {m.course_unit_badge_today()}
      </span>
    {/if}

    {#if dateError}
      <p class="w-full text-xs text-destructive" role="alert" data-testid="unit-date-error">
        {dateError}
      </p>
    {/if}
  </div>
{/if}
