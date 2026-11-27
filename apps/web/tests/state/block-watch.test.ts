/** Whether the reader watches a card (`BlockWatch`), on the backend in memory. */
import { describe, expect, it, vi } from 'vitest';

import { BackendError, BlockWatch } from '$lib/state/index.ts';

import { openFirstBoard, settle } from './support.ts';

describe('watching a card', () => {
  it('reads that the writer of a card watches it, and stops and starts again', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const card = await backend.createCard(board.boardId, { title: 'Mine' });
    const watch = new BlockWatch(workspace, card.id, 'card');
    expect(watch.canWatch).toBe(true);
    expect(watch.watching).toBeUndefined();
    await watch.read();
    expect(watch.watching).toBe(true);

    await watch.set(false);
    expect(watch.watching).toBe(false);
    const meId = workspace.me?.id ?? '';
    const workspaceId = workspace.info?.id ?? '';
    expect(await backend.watches.watching(workspaceId, meId)).not.toContain(card.id);
    await watch.set(true);
    expect(await backend.watches.watching(workspaceId, meId)).toContain(card.id);
  });

  it('goes back and reports when the server refuses', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const card = await backend.createCard(board.boardId, { title: 'Mine' });
    const watch = new BlockWatch(workspace, card.id, 'card');
    await watch.read();
    backend.failNext('watches', new BackendError(500, { detail: 'down' }));
    await watch.set(false);
    await settle();
    expect(watch.watching).toBe(true);
    expect(workspace.problems).toHaveLength(1);
  });

  it('reads the list once for every card of the workspace, and tells each of a switch', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const mine = await backend.createCard(board.boardId, { title: 'Mine' });
    const other = await backend.createCard(board.boardId, { title: 'Other' });
    const reads = vi.spyOn(backend.watches, 'watching');
    const first = new BlockWatch(workspace, mine.id, 'card');
    const again = new BlockWatch(workspace, mine.id, 'card');
    const second = new BlockWatch(workspace, other.id, 'card');
    await Promise.all([first.read(), again.read(), second.read()]);
    await new BlockWatch(workspace, other.id, 'card').read();
    expect(reads).toHaveBeenCalledOnce();

    await first.set(false);
    expect(again.watching).toBe(false);
    expect(second.watching).toBe(true);
    expect(reads).toHaveBeenCalledOnce();
  });

  it('keeps working when the reader is read again as another object of the same account', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const card = await backend.createCard(board.boardId, { title: 'Mine' });
    const watch = new BlockWatch(workspace, card.id, 'card');
    await watch.read();
    const me = workspace.me;
    const info = workspace.info;
    if (!me || !info) throw new Error('the workspace is not entered');
    workspace.me = { ...me };
    workspace.info = { ...info };
    expect(watch.watching).toBe(true);

    backend.failNext('watches', new BackendError(500, { detail: 'down' }));
    await watch.set(false);
    expect(watch.watching).toBe(true);
    expect(workspace.problems).toHaveLength(1);
  });

  it('reads the list again when the connection comes back, a switch made meanwhile winning', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const card = await backend.createCard(board.boardId, { title: 'Mine' });
    const watch = new BlockWatch(workspace, card.id, 'card');
    await watch.read();
    const meId = workspace.me?.id ?? '';
    const workspaceId = workspace.info?.id ?? '';
    // Ended elsewhere while the connection was down: nothing told this page.
    await backend.watches.unwatch(workspaceId, card.id, meId);
    expect(watch.watching).toBe(true);
    const reads = vi.spyOn(backend.watches, 'watching');

    backend.world.tellUsers([meId], { kind: 'resync' });
    await settle();
    expect(reads).toHaveBeenCalledOnce();
    expect(watch.watching).toBe(false);
  });

  it('shows nothing of one workspace’s list in another', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const card = await backend.createCard(board.boardId, { title: 'Mine' });
    const watch = new BlockWatch(workspace, card.id, 'card');
    await watch.read();
    const info = workspace.info;
    if (!info) throw new Error('the workspace is not entered');
    workspace.info = { ...info, id: 'elsewhere' };
    expect(watch.watching).toBeUndefined();
    workspace.info = info;
    expect(watch.watching).toBe(true);
  });
});
