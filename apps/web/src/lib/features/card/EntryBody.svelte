<!--
  The body of the entry of a day of a diary, open on its page: the card's
  kind of body (`CardBody`), in the entry's own room.

  A day without an entry has an editor of its own (the draft): the entry is
  made by what the reader writes first, not before (`EntryStore.write`), and
  the room's editor takes over once the room is there and may be written in,
  with what the draft holds put into it (after what another device wrote
  there meanwhile), the caret where it was. It takes over between syllables,
  never in the middle of one an input method is composing. A draft opened
  again holds what the day kept of it, and is not written in while that is
  being written as the body (`EntryStore.flushing`): the room's editor waits
  for that write, which puts it in the room.

  A day the reader opened by pressing it (`focus`) has the caret put at the
  end of what it holds, once, by whichever editor comes first, and at once:
  what is typed right after the press goes into it.

  Both editors put and draw references (`[[`) to the cards, pages and units
  of the reader's own space, as the bodies of its boards and wikis do
  (`bodyReferences`).
-->
<script lang="ts">
  import type { Editor as TiptapEditor } from '@tiptap/core';
  import { tick } from 'svelte';

  import type { DocNode } from '$lib/editor/schema.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { EntryStore } from '$lib/state/index.ts';

  import { bodyReferences } from './body-references.ts';
  import CardBody from './CardBody.svelte';
  import { entryVersions, keepVersion, type KeepOutcome } from './body-versions.svelte.ts';
  import { editorChunk } from './editor-chunk.svelte.ts';

  interface Props {
    entry: EntryStore;
    /** What the draft's editor is called: the day it is of. */
    label: string;
    /** The reader opened the day: the caret goes into it. */
    focus?: boolean;
  }

  let { entry, label, focus = false }: Props = $props();

  /** Who may write, for the body: the reader alone, in their own diary. */
  const host = $derived({ workspace: entry.host.workspace, canEdit: true, orderedMembers: [] });
  const references = $derived(bodyReferences({ workspace: entry.host.workspace }));
  const Draft = $derived(editorChunk.component);
  $effect(() => {
    editorChunk.load();
  });

  /** The draft's editor is composing a syllable: the room's editor waits. */
  let composing = $state(false);
  /** Where the caret of the draft was, and whether it had the focus; read as the room's editor comes. */
  let caret = { at: 1, focused: false };

  /** The editor now mounted (draft or room), to place the caret once it is editable. */
  let active = $state.raw<TiptapEditor | undefined>(undefined);
  /** The caret was put in as the day was opened. */
  let placed = false;

  function place(editor: TiptapEditor): void {
    if (!focus || placed || !editor.isEditable || editor.isDestroyed) return;
    placed = true;
    // At once, not a frame later as the editor's own command does: keys pressed
    // right after the day was opened would go nowhere until that frame.
    if (editorChunk.selectEnd) editorChunk.selectEnd(editor);
    else editor.chain().setTextSelection(editor.state.doc.content.size).scrollIntoView().run();
    editor.view.focus();
  }

  // An editor mounted read-only (while the room joins or a draft flushes) takes the
  // caret once it becomes editable: after its own effect has set `editor.isEditable`.
  $effect(() => {
    const editor = active;
    const ready = entry.room ? entry.room.writable : !entry.flushing;
    if (!focus || placed || !editor || !ready) return;
    if (editor.isEditable) {
      place(editor);
      return;
    }
    let cancelled = false;
    void tick().then(() => {
      if (cancelled || active !== editor) return;
      // Do not steal focus if the reader moved into another field while the room was joining.
      const focused = document.activeElement;
      if (
        focused instanceof HTMLElement &&
        focused.isConnected &&
        focused !== document.body &&
        !editor.view.dom.contains(focused)
      ) {
        placed = true;
        return;
      }
      place(editor);
    });
    return () => {
      cancelled = true;
    };
  });

  function watchDraft(editor: TiptapEditor): void {
    active = editor;
    let going = false;
    const keep = (): void => {
      if (!going) caret = { at: editor.state.selection.from, focused: editor.isFocused };
    };
    for (const event of ['selectionUpdate', 'update', 'focus'] as const) editor.on(event, keep);
    // A draft taken down is left first, then destroyed, at once: that is not the reader leaving it.
    editor.on('blur', () => {
      queueMicrotask(keep);
    });
    editor.on('destroy', () => {
      going = true;
      if (active === editor) active = undefined;
    });
    place(editor);
  }

  /** The room's editor is there: the draft goes into it, and the caret with it. */
  function adopt(editor: TiptapEditor): void {
    active = editor;
    editor.on('destroy', () => {
      if (active === editor) active = undefined;
    });
    const blank = (): void => {
      entry.noteBlank(editor.isEmpty);
    };
    editor.on('update', blank);
    blank();
    const draft = entry.takeDraft();
    if (!draft) {
      place(editor);
      return;
    }
    placed = true;
    const empty = editor.isEmpty;
    if (empty) editor.commands.setContent(draft);
    else editor.commands.insertContentAt(editor.state.doc.content.size, draft.content ?? []);
    blank();
    if (!caret.focused) return;
    // At once, not a frame later as the editor's own command does: the reader is typing.
    editor.commands.setTextSelection(empty ? caret.at : editor.state.doc.content.size);
    editor.view.focus();
  }

  function keepEntry(): Promise<KeepOutcome> {
    if (!entry.entry) return Promise.resolve(entry.draft === null ? 'unchanged' : 'opening');
    if (entry.drafting || entry.flushing) return Promise.resolve('opening');
    return keepVersion(entryVersions(entry));
  }
</script>

{#snippet draft(onSave: () => void)}
  {#if Draft}
    <Draft
      doc={entry.draft}
      editable={!entry.flushing}
      {label}
      placeholder={m.diary_entry_placeholder()}
      class="min-h-24 text-base outline-none"
      onChange={(doc: DocNode | null) => {
        entry.write(doc);
      }}
      onComposing={(now: boolean) => {
        composing = now;
      }}
      {onSave}
      {references}
      onEditor={watchDraft}
      uploadFile={(file: File) => entry.uploadFile(file)}
    />
  {/if}
{/snippet}

<CardBody
  container={host}
  card={entry}
  of="entry"
  testId="entry-body"
  keep={keepEntry}
  waiting={entry.entry && !entry.drafting ? undefined : draft}
  holding={composing || entry.flushing || (entry.drafting && !entry.room?.writable)}
  {references}
  onEditor={adopt}
/>
{#if entry.unsaved}
  <p class="text-sm text-destructive" role="alert">{m.diary_not_saved({ date: label })}</p>
{/if}
