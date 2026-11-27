<!--
  Personal study notes panel for one course unit: question, mistake, and review
  notes visible strictly to the caller. Supports kind filtering, creation,
  inline editing (content and kind), and deletion. A unit without notes shows
  only the button that starts one. Never mounted on public share links or
  chapters; nothing for a reader of an open course who has not joined it (the
  course's banner joins).
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';

  import LockIcon from '@lucide/svelte/icons/lock';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import StickyNoteIcon from '@lucide/svelte/icons/sticky-note';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isComposing } from '$lib/keys.ts';
  import { relativeTime } from '$lib/logic/format.ts';
  import {
    courseReviews,
    UNIT_NOTE_CONTENT_MAX_CHARS,
    UNIT_NOTE_KINDS,
    validateUnitNoteContent,
    type CourseStore,
    type UnitNote,
    type UnitNoteKind,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    course: CourseStore;
    unitId: string;
  }

  let { course, unitId }: Props = $props();

  const uid = $props.id();
  const store = $derived(courseReviews(course));
  const canTrack = $derived(course.canTrackProgress);
  const locale = $derived(course.workspace.locale);

  const KIND_BADGE_CLASS: Record<UnitNoteKind, string> = {
    question: 'bg-option-blue text-foreground border-border',
    mistake: 'bg-option-red text-foreground border-border',
    review: 'bg-option-green text-foreground border-border',
  };

  function kindLabel(kind: UnitNoteKind): string {
    switch (kind) {
      case 'question':
        return m.unit_notes_kind_question();
      case 'mistake':
        return m.unit_notes_kind_mistake();
      case 'review':
        return m.unit_notes_kind_review();
    }
  }

  function contentErrorText(raw: string): string {
    const problem = validateUnitNoteContent(raw);
    if (problem === 'empty') return m.unit_notes_error_empty();
    if (problem === 'too_long') {
      return m.unit_notes_error_too_long({ max: UNIT_NOTE_CONTENT_MAX_CHARS });
    }
    if (problem === 'invalid_characters') return m.unit_notes_error_invalid_chars();
    return '';
  }

  let filterKind = $state<UnitNoteKind | 'all'>('all');
  let composerKind = $state<UnitNoteKind>('question');
  let draftState = $state<{ unitId: string; text: string }>({ unitId: '', text: '' });
  let composerErrorState = $state<{ unitId: string; text: string } | undefined>(undefined);
  const draft = $derived(draftState.unitId === unitId ? draftState.text : '');
  const composerError = $derived(
    composerErrorState?.unitId === unitId ? composerErrorState.text : '',
  );

  let editing = $state<{ unitId: string; noteId: string } | undefined>(undefined);
  let editKind = $state<UnitNoteKind>('question');
  let editContent = $state('');
  let editError = $state('');
  const editingId = $derived(editing?.unitId === unitId ? editing.noteId : undefined);

  const unitNotes = $derived(store.notesUnitId === unitId ? store.notes : []);
  /** The unit whose first note is being written. */
  let startedFor = $state<string | undefined>(undefined);
  const shown = $derived(
    unitNotes.length > 0 || startedFor === unitId || store.notesStatus === 'failed',
  );
  const filteredNotes = $derived(
    filterKind === 'all' ? unitNotes : unitNotes.filter((note) => note.kind === filterKind),
  );

  $effect(() => {
    const active = canTrack;
    const target = unitId;
    const current = store;
    untrack(() => {
      if (active) void current.loadNotes(target);
      else current.clearNotes();
    });
  });

  async function startNote(): Promise<void> {
    startedFor = unitId;
    await tick();
    document.getElementById(`${uid}-composer`)?.focus();
  }

  function setFilter(next: UnitNoteKind | 'all'): void {
    filterKind = next;
    if (next !== 'all') composerKind = next;
  }

  function setDraft(text: string): void {
    draftState = { unitId, text };
    if (composerError !== '') {
      const nextErr = contentErrorText(text);
      composerErrorState = nextErr ? { unitId, text: nextErr } : undefined;
    }
  }

  async function submitNew(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!canTrack || store.notesBusy) return;
    const targetId = unitId;
    const content = draft;
    const problem = contentErrorText(content);
    if (problem !== '') {
      composerErrorState = { unitId: targetId, text: problem };
      return;
    }
    composerErrorState = undefined;
    const created = await store.createNote(targetId, { kind: composerKind, content });
    if (created && draftState.unitId === targetId) draftState = { unitId: targetId, text: '' };
  }

  function startEdit(note: UnitNote): void {
    editing = { unitId, noteId: note.id };
    editKind = note.kind;
    editContent = note.content;
    editError = '';
  }

  function cancelEdit(): void {
    editing = undefined;
    editError = '';
  }

  async function submitEdit(event: SubmitEvent, note: UnitNote): Promise<void> {
    event.preventDefault();
    if (store.noteBusyIds.has(note.id)) return;
    const problem = contentErrorText(editContent);
    if (problem !== '') {
      editError = problem;
      return;
    }
    editError = '';
    const trimmed = editContent.trim();
    if (editKind === note.kind && trimmed === note.content) {
      if (editing?.noteId === note.id) cancelEdit();
      return;
    }
    const updated = await store.patchNote(note.id, {
      ...(editKind !== note.kind ? { kind: editKind } : {}),
      ...(trimmed !== note.content ? { content: trimmed } : {}),
    });
    if (updated && editing?.noteId === note.id) cancelEdit();
  }

  function onComposerKeydown(event: KeyboardEvent): void {
    if (isComposing(event)) return;
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault();
      (event.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
    }
  }

  function onEditKeydown(event: KeyboardEvent): void {
    if (isComposing(event)) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      cancelEdit();
    } else if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault();
      (event.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
    }
  }
</script>

{#if canTrack && !shown}
  <div class="border-t pt-4">
    <Button
      variant="ghost"
      size="sm"
      class="text-muted-foreground roomy:min-h-touch"
      data-testid="unit-notes-add"
      onclick={() => void startNote()}
    >
      <PlusIcon class="size-3.5" aria-hidden="true" />
      {m.unit_notes_submit()}
    </Button>
  </div>
{:else if canTrack}
  <section
    class="flex flex-col gap-3 border-t pt-5"
    aria-label={m.unit_notes_title()}
    data-testid="unit-notes-panel"
  >
    <div class="flex flex-wrap items-center gap-2">
      <h2 class="flex items-center gap-1.5 text-base font-semibold">
        <StickyNoteIcon class="size-4 text-muted-foreground" aria-hidden="true" />
        {m.unit_notes_title()}
      </h2>
      <span
        class="inline-flex items-center gap-1 rounded-full border bg-muted/60 px-2 py-0.5 text-xs text-muted-foreground"
        data-testid="unit-notes-private-badge"
      >
        <LockIcon class="size-3" aria-hidden="true" />
        {m.unit_notes_private()}
      </span>
    </div>

    {#if unitNotes.length > 0}
      <div
        class="flex flex-wrap items-center gap-1.5"
        role="group"
        aria-label={m.unit_notes_filter_label()}
      >
        <button
          type="button"
          aria-pressed={filterKind === 'all'}
          data-testid="unit-notes-filter-all"
          class={[
            'min-h-8 rounded-full border px-3 text-xs font-medium transition-colors roomy:min-h-touch',
            filterKind === 'all'
              ? 'border-foreground bg-foreground text-background'
              : 'border-border bg-background text-muted-foreground hover:bg-muted',
          ]}
          onclick={() => {
            setFilter('all');
          }}
        >
          {m.unit_notes_filter_all()}
        </button>
        {#each UNIT_NOTE_KINDS as kind (kind)}
          <button
            type="button"
            aria-pressed={filterKind === kind}
            data-testid="unit-notes-filter-{kind}"
            class={[
              'min-h-8 rounded-full border px-3 text-xs font-medium transition-colors roomy:min-h-touch',
              filterKind === kind
                ? [KIND_BADGE_CLASS[kind], 'ring-1 ring-ring/50']
                : 'border-border bg-background text-muted-foreground hover:bg-muted',
            ]}
            onclick={() => {
              setFilter(kind);
            }}
          >
            {kindLabel(kind)}
          </button>
        {/each}
      </div>
    {/if}

    <form
      class="flex flex-col gap-2 rounded-lg border bg-muted/30 p-3"
      data-testid="unit-note-composer"
      onsubmit={(event) => void submitNew(event)}
    >
      <div
        class="flex flex-wrap items-center gap-1.5"
        role="group"
        aria-label={m.unit_notes_kind_label()}
      >
        {#each UNIT_NOTE_KINDS as kind (kind)}
          <button
            type="button"
            aria-pressed={composerKind === kind}
            disabled={store.notesBusy}
            data-testid="unit-note-kind-{kind}"
            class={[
              'min-h-7 rounded-md border px-2.5 text-xs font-medium transition-colors roomy:min-h-touch',
              composerKind === kind
                ? [KIND_BADGE_CLASS[kind], 'ring-1 ring-ring/50']
                : 'border-border bg-background text-muted-foreground hover:bg-muted',
            ]}
            onclick={() => {
              composerKind = kind;
            }}
          >
            {kindLabel(kind)}
          </button>
        {/each}
      </div>

      <textarea
        id="{uid}-composer"
        value={draft}
        rows="3"
        aria-label={m.unit_notes_input_label()}
        aria-invalid={composerError !== '' ? 'true' : undefined}
        aria-describedby={composerError !== '' ? `${uid}-composer-error` : undefined}
        placeholder={m.unit_notes_placeholder()}
        disabled={store.notesBusy}
        data-testid="unit-note-input"
        class="w-full resize-y rounded-md border border-input-border bg-background px-3 py-2 text-base placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none md:text-sm"
        oninput={(event) => {
          setDraft(event.currentTarget.value);
        }}
        onkeydown={onComposerKeydown}></textarea>

      {#if composerError}
        <p
          id="{uid}-composer-error"
          class="text-xs text-destructive"
          role="alert"
          data-testid="unit-note-error"
        >
          {composerError}
        </p>
      {/if}

      <div class="flex justify-end">
        <Button
          type="submit"
          size="sm"
          disabled={store.notesBusy}
          class="roomy:min-h-touch"
          data-testid="unit-note-submit"
        >
          {m.unit_notes_submit()}
        </Button>
      </div>
    </form>

    {#if store.notesStatus === 'loading' && unitNotes.length === 0}
      <p class="text-sm text-muted-foreground" role="status">{m.app_loading()}</p>
    {:else if store.notesStatus === 'failed'}
      <div class="flex items-center gap-2" role="alert">
        <p class="text-sm text-muted-foreground">{m.app_problem_load()}</p>
        <Button
          variant="outline"
          size="sm"
          class="roomy:min-h-touch"
          onclick={() => void store.loadNotes(unitId)}
        >
          {m.app_retry()}
        </Button>
      </div>
    {:else if unitNotes.length > 0 && filteredNotes.length === 0}
      <p class="text-sm text-muted-foreground" data-testid="unit-notes-empty">
        {m.unit_notes_empty_filtered()}
      </p>
    {:else if filteredNotes.length > 0}
      <ul class="flex flex-col gap-2" data-testid="unit-notes-list">
        {#each filteredNotes as note (note.id)}
          {@const busy = store.noteBusyIds.has(note.id)}
          <li
            class="flex flex-col gap-2 rounded-lg border bg-background p-3"
            data-testid="unit-note-item"
            data-note-id={note.id}
            data-note-kind={note.kind}
          >
            {#if editingId === note.id}
              <form
                class="flex flex-col gap-2"
                data-testid="unit-note-edit-form"
                onsubmit={(event) => void submitEdit(event, note)}
              >
                <div
                  class="flex flex-wrap items-center gap-1.5"
                  role="group"
                  aria-label={m.unit_notes_kind_label()}
                >
                  {#each UNIT_NOTE_KINDS as kind (kind)}
                    <button
                      type="button"
                      aria-pressed={editKind === kind}
                      disabled={busy}
                      data-testid="unit-note-edit-kind-{kind}"
                      class={[
                        'min-h-7 rounded-md border px-2.5 text-xs font-medium transition-colors roomy:min-h-touch',
                        editKind === kind
                          ? [KIND_BADGE_CLASS[kind], 'ring-1 ring-ring/50']
                          : 'border-border bg-background text-muted-foreground hover:bg-muted',
                      ]}
                      onclick={() => {
                        editKind = kind;
                      }}
                    >
                      {kindLabel(kind)}
                    </button>
                  {/each}
                </div>
                <textarea
                  bind:value={editContent}
                  rows="3"
                  aria-label={m.unit_notes_edit()}
                  aria-invalid={editError !== '' ? 'true' : undefined}
                  aria-describedby={editError !== '' ? `${uid}-edit-error` : undefined}
                  disabled={busy}
                  data-testid="unit-note-edit-input"
                  class="w-full resize-y rounded-md border border-input-border bg-background px-3 py-2 text-base focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none md:text-sm"
                  oninput={() => {
                    if (editError !== '') editError = contentErrorText(editContent);
                  }}
                  onkeydown={onEditKeydown}></textarea>
                {#if editError}
                  <p
                    id="{uid}-edit-error"
                    class="text-xs text-destructive"
                    role="alert"
                    data-testid="unit-note-edit-error"
                  >
                    {editError}
                  </p>
                {/if}
                <div class="flex flex-wrap items-center gap-2">
                  <Button
                    type="submit"
                    size="sm"
                    disabled={busy}
                    class="roomy:min-h-touch"
                    data-testid="unit-note-save"
                  >
                    {m.unit_notes_save()}
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={busy}
                    class="roomy:min-h-touch"
                    data-testid="unit-note-cancel"
                    onclick={cancelEdit}
                  >
                    {m.shared_cancel()}
                  </Button>
                </div>
              </form>
            {:else}
              <div class="flex items-start justify-between gap-2">
                <div class="flex flex-wrap items-center gap-2">
                  <span
                    class={[
                      'rounded-full border px-2 py-0.5 text-xs font-medium',
                      KIND_BADGE_CLASS[note.kind],
                    ]}
                    data-testid="unit-note-kind-badge"
                  >
                    {kindLabel(note.kind)}
                  </span>
                  <span class="text-xs text-muted-foreground">
                    {relativeTime(note.updatedAt, locale, Math.max(Date.now(), note.updatedAt))}
                  </span>
                </div>
                <div class="flex shrink-0 items-center gap-0.5">
                  <IconButton
                    label={m.unit_notes_edit()}
                    disabled={busy}
                    data-testid="unit-note-edit"
                    class="size-7 text-muted-foreground hover:text-foreground"
                    onclick={() => {
                      startEdit(note);
                    }}
                  >
                    <PencilIcon class="size-3.5" />
                  </IconButton>
                  <IconButton
                    label={m.unit_notes_delete()}
                    disabled={busy}
                    data-testid="unit-note-delete"
                    class="size-7 text-muted-foreground hover:text-destructive"
                    onclick={() => void store.deleteNote(note.id)}
                  >
                    <Trash2Icon class="size-3.5" />
                  </IconButton>
                </div>
              </div>
              <p
                class="text-sm break-words whitespace-pre-wrap text-foreground"
                data-testid="unit-note-content"
              >
                {note.content}
              </p>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}
