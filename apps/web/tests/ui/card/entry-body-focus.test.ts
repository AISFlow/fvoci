/**
 * Focus placement in `EntryBody.svelte`: an editor mounted while its room is
 * not yet writable does not consume `placed`, and moves the caret to the end
 * with focus once the room becomes writable.
 */
import type { Editor } from '@tiptap/core';
import { render } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';

import '$lib/editor/BodyEditor.svelte';
import EntryBody from '$lib/features/card/EntryBody.svelte';
import { dateOfUtc } from '$lib/logic/dates.ts';
import { monthDayOf } from '$lib/logic/diary-days.ts';
import type { BodyDocHears, BodyDocState } from '$lib/state/backend.ts';
import { DiaryStore, firstDiary } from '$lib/state/index.ts';
import '$lib/state/memory/rooms.ts';

import { setup, text } from '../../state/support.ts';

afterEach(() => {
  document.body.replaceChildren();
});

it('waits until the room editor becomes editable before placing the caret at the end', async () => {
  const { backend, workspace, world } = setup();
  const own = (await backend.workspaces()).find((one) => one.kind === 'personal');
  if (!own) throw new Error('no own space');
  await workspace.load(own.id);
  const diary = await firstDiary(workspace);
  if (!diary) throw new Error('no diary');
  const today = dateOfUtc(world.now());
  const made = await backend.diary.createEntry(diary.id, today);
  await backend.diary.patchEntry(diary.id, made.id, {
    body: text('Hello world'),
    readAt: made.updatedAt,
  });

  // Hold the room read-only on first join, then release it to writable.
  let holdReadOnly = true;
  let notify: ((state: BodyDocState) => void) | undefined;
  let latest: BodyDocState | undefined;
  const openDoc = backend.openDoc.bind(backend);
  vi.spyOn(backend, 'openDoc').mockImplementation(
    (workspaceId, containerId, cardId, hears: BodyDocHears) => {
      notify = (state) => {
        hears.state(state);
      };
      return openDoc(workspaceId, containerId, cardId, {
        ...hears,
        state: (state) => {
          latest = state;
          hears.state(holdReadOnly ? { ...state, writable: false } : state);
        },
      });
    },
  );

  const store = new DiaryStore(workspace, diary.id);
  await store.show(monthDayOf(today));
  const entry = store.day(today);
  const { container, unmount } = render(EntryBody, {
    entry,
    label: 'November 21, 2026',
    focus: true,
  });
  try {
    await vi.waitFor(() => {
      expect(
        container.querySelector('[data-testid="entry-body"]')?.getAttribute('data-editor'),
      ).toBe('reading');
    });
    const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
    if (!root?.editor) throw new Error('no editor');
    expect(root.editor.isEditable).toBe(false);
    expect(root.editor.isFocused).toBe(false);

    // Once writable, `EntryBody` retries `place()` and puts the caret at the end with focus.
    holdReadOnly = false;
    if (!latest || !notify) throw new Error('no room state');
    notify(latest);
    await vi.waitFor(() => {
      expect(
        container.querySelector('[data-testid="entry-body"]')?.getAttribute('data-editor'),
      ).toBe('ready');
      expect(root.editor?.isEditable).toBe(true);
      expect(document.activeElement).toBe(root);
    });
    // End of the single paragraph "Hello world" (pos 12, inside closing tag at content.size - 1).
    expect(root.editor.state.selection.from).toBe(root.editor.state.doc.content.size - 1);
  } finally {
    unmount();
    store.close();
  }
});

it('places the selection cleanly at the end of an entry ending with a horizontal rule', async () => {
  const { backend, workspace, world } = setup();
  const own = (await backend.workspaces()).find((one) => one.kind === 'personal');
  if (!own) throw new Error('no own space');
  await workspace.load(own.id);
  const diary = await firstDiary(workspace);
  if (!diary) throw new Error('no diary');
  const today = dateOfUtc(world.now());
  const made = await backend.diary.createEntry(diary.id, today);
  await backend.diary.patchEntry(diary.id, made.id, {
    body: {
      type: 'doc',
      content: [
        { type: 'paragraph', content: [{ type: 'text', text: 'Before' }] },
        { type: 'horizontalRule' },
      ],
    },
    readAt: made.updatedAt,
  });

  const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
  const store = new DiaryStore(workspace, diary.id);
  await store.show(monthDayOf(today));
  const entry = store.day(today);
  const { container, unmount } = render(EntryBody, {
    entry,
    label: 'November 21, 2026',
    focus: true,
  });
  try {
    await vi.waitFor(() => {
      expect(
        container.querySelector('[data-testid="entry-body"]')?.getAttribute('data-editor'),
      ).toBe('ready');
      const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
      expect(document.activeElement).toBe(root);
    });
    const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
    if (!root?.editor) throw new Error('no editor');
    expect(root.editor.state.selection.to).toBe(root.editor.state.doc.content.size);
    expect(warn).not.toHaveBeenCalled();
  } finally {
    warn.mockRestore();
    unmount();
    store.close();
  }
});
