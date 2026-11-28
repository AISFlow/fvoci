/**
 * The reactions under a comment (CommentReactions.svelte): an emoji with its
 * count, the reader's own pressed, named by who reacted ("you", the people
 * the reader knows, "someone" for the rest); pressed, it comes off, and one
 * of the quick row goes on.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';

import CommentReactions from '$lib/features/card/CommentReactions.svelte';

import { openFirstBoard, settle, text } from '../../state/support.ts';

afterEach(cleanup);

describe('the reactions under a comment', () => {
  it('name who reacted, toggle the reader’s own, and add one of the quick row', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const made = await backend.createCard(board.boardId, { title: 'Mine' });
    const comment = await backend.createComment(board.boardId, made.id, text('hello'));
    const reactions = backend.reactions;
    const workspaceId = workspace.info?.id;
    if (!workspaceId) throw new Error('the workspace is not entered');
    await reactions.add(workspaceId, board.boardId, comment.id, '👍');
    // Somebody the reader is told nothing of.
    backend.world.reactions.set(comment.id, [
      ...(backend.world.reactions.get(comment.id) ?? []),
      { emoji: '👀', userIds: ['nobody-known'] },
    ]);
    const card = await board.showCard(made.id);
    await settle();

    render(CommentReactions, { props: { reactions: card.reactions, blockId: comment.id } });
    const group = await screen.findByRole('group', { name: 'Reactions' });
    const thumbs = within(group).getByRole('button', { name: /^👍/ });
    expect(thumbs).toHaveAttribute('aria-pressed', 'true');
    expect(thumbs.getAttribute('aria-label')).toMatch(/you/);
    const eyes = within(group).getByRole('button', { name: /^👀 1: someone$/ });
    expect(eyes).toHaveAttribute('aria-pressed', 'false');

    await fireEvent.click(thumbs);
    await waitFor(() => {
      const left = within(group).queryByRole('button', { name: /^👍/ });
      expect(left?.getAttribute('aria-pressed') ?? 'false').toBe('false');
    });

    await fireEvent.click(within(group).getByRole('button', { name: 'Add a reaction' }));
    const quick = await screen.findByRole('group', { name: 'Quick reactions' });
    await fireEvent.click(within(quick).getByRole('button', { name: '🎉' }));
    await waitFor(() => {
      expect(within(group).getByRole('button', { name: /^🎉 1: you/ })).toHaveAttribute(
        'aria-pressed',
        'true',
      );
    });
    await settle();
    const stored = backend.world.reactions.get(comment.id) ?? [];
    expect(stored.find((one) => one.emoji === '🎉')?.userIds).toEqual([backend.userId]);
  });
});
