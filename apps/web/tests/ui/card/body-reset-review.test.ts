/** A new room epoch must also account for drafts held by formula fields. */
import type { Editor } from '@tiptap/core';
import { render } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';

// Loaded with this file, before the test's clock runs. CardBody asks for the editor's
// code when the room opens (editor-chunk.svelte.ts); reading it for the first time takes
// a loaded machine longer than any wait of the test, and here it is already read.
import '$lib/editor/BodyEditor.svelte';
import CardBody from '$lib/features/card/CardBody.svelte';

import { openFirstBoard } from '../../state/support.ts';
import { doc, math, paragraph, text } from '../../support/body-docs.ts';

it('says that an inline formula draft was lost when the room receives a new epoch', async () => {
  const { board, other } = await openFirstBoard();
  const summary = board.cards.find((card) => card.title === 'Alpha');
  if (!summary) throw new Error('no card');
  const before = await other.card(board.boardId, summary.id);
  await other.writeCardBody(board.boardId, summary.id, {
    body: doc(paragraph(math('x'))),
    readAt: before.updatedAt,
  });
  const card = await board.showCard(summary.id);
  const { container, unmount } = render(CardBody, { container: board, card });
  try {
    await vi.waitFor(() => {
      expect(
        container.querySelector('[data-testid="card-body"]')?.getAttribute('data-editor'),
      ).toBe('ready');
    });
    const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
    if (!root?.editor) throw new Error('no editor');
    root.editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    const field = document.querySelector<HTMLTextAreaElement>('.doc-math-editing textarea');
    if (!field) throw new Error('no formula field');
    field.value = 'my-unsubmitted-formula';
    field.dispatchEvent(new Event('input', { bubbles: true }));
    expect(card.bodyPending).toBe(false);

    const current = await other.card(board.boardId, summary.id);
    await other.writeCardBody(board.boardId, summary.id, {
      body: doc(paragraph(text('replacement from the API'))),
      readAt: current.updatedAt,
    });
    await vi.waitFor(() => {
      expect(container.querySelector('.ProseMirror')?.textContent).toBe('replacement from the API');
      expect(field.isConnected).toBe(false);
    });
    expect(container.querySelector('[data-testid="body-lost"]')).not.toBeNull();
    // Said as the formula's: nothing else of the reader's was waiting.
    expect(card.lost).toEqual(['formula_dropped']);
  } finally {
    unmount();
    board.close();
  }
});

it('shows the dedicated 200x20 notice when a collaborative table edit is refused with table_too_large', async () => {
  const { board } = await openFirstBoard();
  const summary = board.cards.find((card) => card.title === 'Alpha');
  if (!summary) throw new Error('no card');
  const card = await board.showCard(summary.id);
  const { container, unmount } = render(CardBody, { container: board, card });
  try {
    card.sayLost('table_too_large');
    await vi.waitFor(() => {
      const alert = container.querySelector('[data-testid="body-lost"]');
      expect(alert?.textContent).toContain('200');
      expect(alert?.textContent).toContain('20');
    });
  } finally {
    unmount();
    board.close();
  }
});
