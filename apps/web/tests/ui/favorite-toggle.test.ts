/**
 * The ★ (`FavoriteToggle`) reads the reader's stars where it is shown, and
 * again when the workspace changes under it: entered from another, its
 * stars are that workspace's, not none.
 */
import { cleanup, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import FavoriteToggle from '$lib/components/favorite-toggle/FavoriteToggle.svelte';
import { workspaceFavorites } from '$lib/state/index.ts';

import { openFirstBoard } from '../state/support.ts';

afterEach(() => {
  cleanup();
});

describe('the ★', () => {
  it('reads the stars of the workspace it is in, and of another entered', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const first = workspace.info?.id ?? '';
    await backend.favorites.star(first, board.boardId);
    const list = vi.spyOn(backend.favorites, 'list');
    render(FavoriteToggle, {
      props: { stars: workspaceFavorites(workspace), targetId: board.boardId },
    });
    const toggle = await screen.findByTestId('favorite-toggle');
    await waitFor(() => {
      expect(toggle).toHaveAttribute('aria-pressed', 'true');
    });
    expect(list).toHaveBeenLastCalledWith(first);

    list.mockResolvedValueOnce([]);
    workspace.info = { id: 'other-workspace', title: 'Other' };
    await waitFor(() => {
      expect(list).toHaveBeenLastCalledWith('other-workspace');
      expect(screen.getByTestId('favorite-toggle')).toHaveAttribute('aria-pressed', 'false');
    });
  });
});
