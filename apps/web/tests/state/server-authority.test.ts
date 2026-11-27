/**
 * Client state tests for `server-authority` contracts:
 * - Caller-scoped `myPermissions` isolation across `containerChanged` & `workspaceRoleChanged`
 * - Atomic `moveViewCards` (`dropCards`), `batchDeleteCards`, and `batchRestoreCards`
 * - Cross-workspace container copy (`copyContainerToWorkspace` & `allowCopyOut`)
 * - Unified `'Today'` hub (`TodayHubStore` over `accountToday` + client `diaryRuns`)
 */
import { describe, expect, it, vi } from 'vitest';

import {
  BackendError,
  TodayHubStore,
  Workspace,
  isBackendError,
  isOwnSpace,
} from '$lib/state/index.ts';

import { openFirstBoard, settle, STATUS } from './support.ts';

describe('caller-scoped myPermissions isolation', () => {
  it('never leaks an admin’s permissions to a viewer on containerChanged, and refreshes on workspaceRoleChanged', async () => {
    const made = await openFirstBoard();
    const { board: adminBoard, other, world } = made;
    await made.backend.setMemberRole(adminBoard.boardId, other.userId, 'viewer');

    const viewerWs = new Workspace({ backend: other, locale: () => 'en' });
    await viewerWs.load();
    const viewerBoard = await viewerWs.openBoard(adminBoard.boardId);

    expect([adminBoard.canEdit, adminBoard.canManageRoles]).toEqual([true, true]);
    expect([viewerBoard.canEdit, viewerBoard.canManageRoles]).toEqual([false, false]);

    // Admin updates the board container -> broadcasts containerChanged to all readers.
    await adminBoard.changeBoard((cur) => ({ ...cur, title: 'Renamed by Admin' }));
    await settle();
    expect(viewerBoard.board?.title).toBe('Renamed by Admin');
    expect([viewerBoard.canEdit, viewerBoard.canManageRoles]).toEqual([false, false]);

    // Workspace role of viewer changes to guest -> refreshes workspace role & permissions cleanly.
    const wsId = world.workspace.id;
    world.tellUsers([other.userId], {
      kind: 'workspaceRole',
      workspaceId: wsId,
      userId: other.userId,
      role: 'guest',
    });
    await settle();
    expect(viewerWs.info?.role).toBe('guest');
    expect(viewerBoard.canShare).toBe(false);
    viewerWs.close();
  });
});

describe('atomic multi-card move, batch delete, and batch restore', () => {
  it('moves multiple cards across columns in one moveViewCards call and undoes cleanly', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const view = board.views[0];
    const [alpha, beta] = board.realCards;
    if (!view || !alpha || !beta) throw new Error('missing view or cards');

    const moveSpy = vi.spyOn(backend, 'moveViewCards');
    await board.dropCards({
      viewId: view.id,
      cardIds: [alpha.id, beta.id],
      optionId: 'done',
      index: 0,
    });
    expect(moveSpy).toHaveBeenCalledTimes(1);
    expect(board.card(alpha.id)?.fields.properties[STATUS]).toBe('done');
    expect(board.card(beta.id)?.fields.properties[STATUS]).toBe('done');

    await workspace.undoLast();
    expect(board.card(alpha.id)?.fields.properties[STATUS]).toBe('todo');
    expect(board.card(beta.id)?.fields.properties[STATUS]).toBe('todo');
  });

  it('batch-deletes and batch-restores multiple cards in one undo step', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const ids = board.realCards.slice(0, 2).map((c) => c.id);
    const remainingIds = board.realCards.slice(2).map((c) => c.id);
    const delSpy = vi.spyOn(backend, 'batchDeleteCards');
    const restoreSpy = vi.spyOn(backend, 'batchRestoreCards');

    await board.deleteCards(ids);
    expect(delSpy).toHaveBeenCalledWith(board.boardId, ids);
    expect(board.realCards.map((c) => c.id)).toEqual(remainingIds);

    await workspace.undoLast();
    expect(restoreSpy).toHaveBeenCalledWith(board.boardId, ids);
    expect(board.realCards).toHaveLength(ids.length + remainingIds.length);
  });
});

describe('cross-workspace copy and organization copy-out policy', () => {
  it('copies a board to the reader’s personal space when allowCopyOut is true and refuses 403 when false', async () => {
    const { board, backend, workspace, world } = await openFirstBoard();
    const ownWs = workspace.workspaces.find((ws) => isOwnSpace(ws));
    if (!ownWs) throw new Error('missing personal workspace');

    const copied = await workspace.copyContainerToWorkspace(board.boardId, {
      targetWorkspaceId: ownWs.id,
      title: 'Personal Copy',
    });
    expect(copied.container.title).toBe('Personal Copy');
    expect(copied.container.workspaceId).toBe(ownWs.id);

    world.allowCopyOut = false;
    const refused = await backend
      .copyContainerToWorkspace(board.boardId, { targetWorkspaceId: ownWs.id })
      .catch((err: unknown) => err);
    expect(isBackendError(refused) && [refused.status, refused.reason]).toEqual([
      403,
      'copy_out_disabled',
    ]);
  });
});

describe('TodayHubStore', () => {
  it('aggregates due cards, diary status, and lockedWorkspaceCount across workspaces', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    await board.changeBoard((cur) => ({
      ...cur,
      dueDatePropertyId: 'due',
      cardProperties: [
        ...cur.cardProperties,
        { id: 'owner', name: 'Owner', type: 'person', options: [] },
        { id: 'due', name: 'Due', type: 'date', options: [] },
      ],
    }));
    const todayIso = (await backend.accountToday()).today;
    const alpha = board.realCards[0];
    if (!alpha) throw new Error('missing card');
    const me = workspace.me?.id ?? '';
    await backend.patchCard(board.boardId, alpha.id, {
      updatedProperties: { owner: me, due: { from: todayIso } },
    });

    const hub = new TodayHubStore(workspace);
    await hub.load();
    expect(hub.status).toBe('ready');
    expect(hub.data?.cards.map((item) => item.cardId)).toContain(alpha.id);
    expect(hub.data?.lockedWorkspaceCount).toBe(0);

    vi.spyOn(backend, 'accountToday').mockRejectedValueOnce(new BackendError(503));
    await hub.load();
    expect(workspace.problems.at(-1)?.action).toBe('load');
    hub.close();
  });
});
