<!--
  Inner form for creating or editing a personal time entry.
  Mounted only while `TimeEntryDialog` is open so initial state comes directly
  from props with no syncing `$effect`.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    fromDateTimeInput,
    TIME_ENTRY_KINDS,
    TIME_NOTE_MAX_CHARS,
    toDateTimeInput,
    type TimeEntryKind,
  } from '$lib/logic/time.ts';
  import {
    isBackendError,
    isPasswordSignInRequired,
    isSignedOut,
    ssoOrgOf,
    type CreateTimeEntryRequest,
    type TimeEntry,
    type TimeEntryPatch,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { Textarea } from '$lib/ui/textarea/index.js';

  import { KIND_BADGE_CLASS, kindLabel } from './time-labels.ts';

  interface Props {
    mode: 'create' | 'edit';
    entry?: TimeEntry | undefined;
    timeZone: string;
    onCreate: (request: CreateTimeEntryRequest) => Promise<void>;
    onPatch: (entryId: string, patch: TimeEntryPatch) => Promise<void>;
    onCancel: () => void;
    onSaved: () => void;
  }

  let { mode, entry, timeZone, onCreate, onPatch, onCancel, onSaved }: Props = $props();

  const uid = $props.id();
  const initialNow = Date.now();
  const initialStart = untrack(() => (entry ? entry.startedAt : initialNow - 30 * 60 * 1000));
  const initialEnd = untrack(() => (entry ? entry.endedAt : initialNow));

  const initialZone = untrack(() => timeZone);
  const initialStartedInput = toDateTimeInput(initialStart, initialZone);
  const initialEndedInput = initialEnd !== null ? toDateTimeInput(initialEnd, initialZone) : '';

  let kind = $state<TimeEntryKind>(untrack(() => entry?.kind ?? 'study'));
  let startedInput = $state(initialStartedInput);
  let endedInput = $state(initialEndedInput);
  let note = $state(untrack(() => entry?.note ?? ''));
  let saving = $state(false);
  let errorText = $state('');

  const isRunningEdit = $derived(mode === 'edit' && entry?.endedAt === null);
  const noteLength = $derived(Array.from(note).length);

  function validate(): { startedAt: number; endedAt: number | null } | undefined {
    const startUnchanged = mode === 'edit' && entry && startedInput === initialStartedInput;
    const startedAt = startUnchanged ? entry.startedAt : fromDateTimeInput(startedInput, timeZone);
    if (startedAt === undefined) {
      errorText = m.time_error_invalid_start();
      return undefined;
    }
    let endedAt: number | null = null;
    const endUnchanged = mode === 'edit' && entry && endedInput === initialEndedInput;
    if (endUnchanged) {
      endedAt = entry.endedAt;
    } else if (endedInput.trim() === '') {
      if (!isRunningEdit) {
        errorText = mode === 'edit' ? m.time_error_reopen_forbidden() : m.time_error_invalid_end();
        return undefined;
      }
    } else {
      const parsedEnd = fromDateTimeInput(endedInput, timeZone);
      if (parsedEnd === undefined) {
        errorText = m.time_error_invalid_end();
        return undefined;
      }
      endedAt = parsedEnd;
    }
    if (endedAt !== null && endedAt < startedAt) {
      errorText = m.time_error_end_before_start();
      return undefined;
    }
    if (noteLength > TIME_NOTE_MAX_CHARS) {
      errorText = m.time_error_note_too_long({ limit: TIME_NOTE_MAX_CHARS });
      return undefined;
    }
    return { startedAt, endedAt };
  }

  function serverErrorMessage(error: unknown): string {
    if (isBackendError(error) && error.status === 400) {
      if (error.refusalAt('/endedAt') || error.refusalAt('/startedAt')) {
        return m.time_error_end_before_start();
      }
      if (error.refusalAt('/note')?.reason === 'too_long') {
        return m.time_error_note_too_long({ limit: TIME_NOTE_MAX_CHARS });
      }
    }
    return m.app_problem_save();
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (saving) return;
    errorText = '';
    const valid = validate();
    if (!valid) return;

    saving = true;
    try {
      if (mode === 'create') {
        if (valid.endedAt === null) {
          errorText = m.time_error_invalid_end();
          return;
        }
        await onCreate({
          kind,
          startedAt: valid.startedAt,
          endedAt: valid.endedAt,
          ...(note !== '' ? { note } : {}),
        });
      } else if (entry) {
        const patch: TimeEntryPatch = {};
        if (kind !== entry.kind) patch.kind = kind;
        if (valid.startedAt !== entry.startedAt) patch.startedAt = valid.startedAt;
        if (valid.endedAt !== null && valid.endedAt !== entry.endedAt) {
          patch.endedAt = valid.endedAt;
        }
        if (note !== entry.note) patch.note = note;
        if (Object.keys(patch).length > 0) await onPatch(entry.id, patch);
      }
      onSaved();
    } catch (error) {
      if (isPasswordSignInRequired(error) || ssoOrgOf(error) !== undefined || isSignedOut(error)) {
        onCancel();
        return;
      }
      errorText = serverErrorMessage(error);
    } finally {
      saving = false;
    }
  }
</script>

<form class="flex flex-col gap-4" onsubmit={(event) => void submit(event)} novalidate>
  <fieldset class="flex flex-col gap-2">
    <legend class="text-sm font-medium">{m.time_field_kind()}</legend>
    <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
      {#each TIME_ENTRY_KINDS as candidate (candidate)}
        <button
          type="button"
          data-testid="time-form-kind-{candidate}"
          aria-pressed={kind === candidate}
          class={[
            'flex min-h-9 items-center justify-center rounded-md border px-2.5 text-sm font-medium transition-colors roomy:min-h-touch',
            kind === candidate
              ? [KIND_BADGE_CLASS[candidate], 'ring-2 ring-ring/50']
              : 'border-border bg-background text-muted-foreground hover:bg-muted',
          ]}
          onclick={() => {
            kind = candidate;
          }}
        >
          {kindLabel(candidate)}
        </button>
      {/each}
    </div>
  </fieldset>

  <div class="grid gap-3 sm:grid-cols-2">
    <div class="flex flex-col gap-1.5">
      <Label for="{uid}-start">{m.time_field_started_at()}</Label>
      <Input
        id="{uid}-start"
        type="datetime-local"
        bind:value={startedInput}
        required
        data-testid="time-form-started-at"
        class="roomy:h-touch"
      />
    </div>
    <div class="flex flex-col gap-1.5">
      <Label for="{uid}-end">{m.time_field_ended_at()}</Label>
      <Input
        id="{uid}-end"
        type="datetime-local"
        bind:value={endedInput}
        required={!isRunningEdit}
        data-testid="time-form-ended-at"
        class="roomy:h-touch"
      />
    </div>
  </div>

  {#if isRunningEdit}
    <p class="text-xs text-muted-foreground">{m.time_field_ended_at_running_hint()}</p>
  {/if}

  <div class="flex flex-col gap-1.5">
    <div class="flex items-center justify-between gap-2">
      <Label for="{uid}-note">{m.time_field_note()}</Label>
      <span
        class={[
          'text-xs tabular-nums',
          noteLength > TIME_NOTE_MAX_CHARS ? 'text-destructive' : 'text-muted-foreground',
        ]}
      >
        {noteLength}/{TIME_NOTE_MAX_CHARS}
      </span>
    </div>
    <Textarea
      id="{uid}-note"
      bind:value={note}
      rows={3}
      placeholder={m.time_field_note_placeholder()}
      data-testid="time-form-note"
      class="resize-none"
    />
  </div>

  <p role="alert" data-testid="time-form-error" class="text-sm text-destructive empty:hidden">
    {errorText}
  </p>

  <div class="flex justify-end gap-2 pt-1">
    <Button
      type="button"
      variant="outline"
      class="roomy:min-h-touch"
      disabled={saving}
      onclick={onCancel}
    >
      {m.shared_cancel()}
    </Button>
    <Button
      type="submit"
      data-testid="time-form-submit"
      class="roomy:min-h-touch"
      disabled={saving}
    >
      {m.time_save()}
    </Button>
  </div>
</form>
