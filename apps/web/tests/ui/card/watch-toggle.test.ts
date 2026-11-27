/**
 * The switch of watching at the head of a card (WatchToggle.svelte): the
 * reader read again as another object of the same account, as a connection
 * that comes back does, leaves it working, and a refused switch goes back.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import WatchToggle from '$lib/features/card/WatchToggle.svelte';
import { BackendError } from '$lib/state/index.ts';

import { openFirstBoard, settle } from '../../state/support.ts';

afterEach(cleanup);

describe('the switch of watching a card', () => {
  it('goes back and is reported when refused after the reader was read again', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const card = await backend.createCard(board.boardId, { title: 'Mine' });
    const reads = vi.spyOn(backend.watches, 'watching');
    render(WatchToggle, { props: { workspace, blockId: card.id, kind: 'card' } });
    const toggle = await screen.findByTestId('watch-toggle');
    expect(toggle).toHaveAttribute('aria-pressed', 'true');

    const me = workspace.me;
    const info = workspace.info;
    if (!me || !info) throw new Error('the workspace is not entered');
    workspace.me = { ...me };
    workspace.info = { ...info };
    flushSync();
    await settle();
    expect(reads).toHaveBeenCalledOnce();

    backend.failNext('watches', new BackendError(500, { detail: 'down' }));
    await fireEvent.click(toggle);
    await settle();
    await waitFor(() => {
      expect(screen.getByTestId('watch-toggle')).toHaveAttribute('aria-pressed', 'true');
    });
    expect(workspace.problems).toHaveLength(1);

    await fireEvent.click(screen.getByTestId('watch-toggle'));
    await waitFor(() => {
      expect(screen.getByTestId('watch-toggle')).toHaveAttribute('aria-pressed', 'false');
    });
    const meId = workspace.me.id;
    expect(await backend.watches.watching(info.id, meId)).not.toContain(card.id);
  });

  it('reads the list once for two cards shown at once', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const one = await backend.createCard(board.boardId, { title: 'One' });
    const two = await backend.createCard(board.boardId, { title: 'Two' });
    const reads = vi.spyOn(backend.watches, 'watching');
    render(WatchToggle, { props: { workspace, blockId: one.id, kind: 'card' } });
    render(WatchToggle, { props: { workspace, blockId: two.id, kind: 'card' } });
    await waitFor(() => {
      expect(screen.getAllByTestId('watch-toggle')).toHaveLength(2);
    });
    expect(reads).toHaveBeenCalledOnce();
  });
});
