<!--
  How a card repeats, chosen under its due date: not at all, every day,
  week, month or year from the due date, or a recurrence of one's own (every
  1 to 365 units, from the due date or from the day it is done). A choice is
  written at once (`onChange`), apart from the date the picker is editing.

  Only a board with a rule of done and a due date property lets a card
  repeat: elsewhere the choices are off and the fields say where to set
  them; a card that repeats there already can still be made to stop.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    anew,
    choiceOf,
    MOST_PERIODS,
    RECURRENCE_CHOICES,
    RECURRENCE_UNITS,
    recurrenceOfChoice,
    sameRecurrence,
    type Recurrence,
    type RecurrenceAnchor,
    type RecurrenceChoice,
    type RecurrenceUnit,
  } from '$lib/logic/recurrence.ts';

  import { UNIT_NAMES } from './recurrence-text.ts';

  interface Props {
    /** How the card repeats now; none for a card that does not. */
    recurrence: Recurrence | undefined;
    /** The board lets its cards repeat. */
    allowed: boolean;
    onChange: (recurrence: Recurrence | undefined) => void;
  }

  let { recurrence, allowed, onChange }: Props = $props();

  const uid = $props.id();
  const CHOICE_NAMES: Record<RecurrenceChoice, () => string> = {
    none: () => m.property_date_repeat_none(),
    daily: () => m.property_date_repeat_daily(),
    weekly: () => m.property_date_repeat_weekly(),
    monthly: () => m.property_date_repeat_monthly(),
    yearly: () => m.property_date_repeat_yearly(),
    custom: () => m.property_date_repeat_custom(),
  };

  /** "Custom" was picked for a recurrence that is one of the others: its fields stay open. */
  let ownOpen = $state(false);
  const choice = $derived<RecurrenceChoice>(
    ownOpen && recurrence ? 'custom' : choiceOf(recurrence),
  );

  function choose(event: Event): void {
    const picked = (event.currentTarget as HTMLSelectElement).value as RecurrenceChoice;
    ownOpen = picked === 'custom';
    const next = recurrenceOfChoice(picked, recurrence);
    if (!sameRecurrence(next, recurrence)) onChange(next);
  }

  /** Repeating otherwise, the card starts a chain of its own. */
  function change(part: Partial<Recurrence>): void {
    if (recurrence) onChange({ ...anew(recurrence), ...part });
  }

  function setEvery(event: Event): void {
    const input = event.currentTarget as HTMLInputElement;
    const typed = Math.round(Number(input.value));
    const every = Number.isFinite(typed) ? Math.min(Math.max(typed, 1), MOST_PERIODS) : 1;
    input.value = String(every);
    if (every !== recurrence?.every) change({ every });
  }

  const fieldClass =
    'h-8 rounded-lg border border-input-border bg-transparent px-1.5 text-base text-foreground outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-50 md:text-sm dark:bg-input/30 group-data-[presentation=sheet]/surface:h-touch group-data-[presentation=sheet]/surface:px-3';
</script>

<div
  role="group"
  aria-labelledby="{uid}-label"
  data-slot="recurrence"
  class="flex flex-col gap-1.5"
>
  <div
    class="flex min-h-8 items-center justify-between gap-2 px-2 group-data-[presentation=sheet]/surface:min-h-touch"
  >
    <label for="{uid}-choice" id="{uid}-label" class="text-sm">{m.property_date_repeat()}</label>
    <select
      id="{uid}-choice"
      class={fieldClass}
      value={choice}
      disabled={!allowed && !recurrence}
      onchange={choose}
    >
      {#each RECURRENCE_CHOICES as each (each)}
        <option value={each} disabled={!allowed && each !== 'none'}>{CHOICE_NAMES[each]()}</option>
      {/each}
    </select>
  </div>
  {#if choice === 'custom' && recurrence}
    <div class="flex flex-wrap items-center justify-end gap-1.5 px-2" data-slot="recurrence-own">
      <input
        type="number"
        inputmode="numeric"
        min="1"
        max={MOST_PERIODS}
        class="{fieldClass} w-16"
        aria-label={m.property_date_repeat_every()}
        value={recurrence.every}
        onchange={setEvery}
      />
      <select
        class={fieldClass}
        aria-label={m.property_date_repeat_unit()}
        value={recurrence.unit}
        onchange={(event) => {
          change({ unit: event.currentTarget.value as RecurrenceUnit });
        }}
      >
        {#each RECURRENCE_UNITS as unit (unit)}
          <option value={unit}>{UNIT_NAMES[unit]()}</option>
        {/each}
      </select>
      <select
        class={fieldClass}
        aria-label={m.property_date_repeat_from()}
        value={recurrence.anchor}
        onchange={(event) => {
          change({ anchor: event.currentTarget.value as RecurrenceAnchor });
        }}
      >
        <option value="due">{m.property_date_repeat_from_due()}</option>
        <option value="done">{m.property_date_repeat_from_done()}</option>
      </select>
    </div>
  {/if}
  {#if !allowed}
    <p class="px-2 text-xs text-muted-foreground" data-slot="recurrence-needs-rules">
      {m.property_date_repeat_needs_rules()}
    </p>
  {:else if recurrence}
    <p class="px-2 text-xs text-muted-foreground">{m.property_date_repeat_hint()}</p>
  {/if}
</div>
