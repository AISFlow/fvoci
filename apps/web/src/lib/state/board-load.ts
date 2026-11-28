/** Reads a board's content and learns the names it carries beyond current membership. */
import type { BoardView, Container, ContainerMember } from '../logic/wire.ts';

import {
  isBackendError,
  READ_ONLY_PERMISSIONS,
  type CardSummary,
  type ContainerPermissions,
} from './backend.ts';
import { assignedIds, authorIds, memberId } from './user-names.ts';
import { containerOfWorkspace } from './board-parts.ts';
import { listOpened } from './join-on-write.ts';
import type { LoadStatus, Workspace } from './workspace.svelte.ts';

/** Target state of a `BoardStore` when a load or refresh fails. */
export interface BoardFailureHost {
  readonly workspace: Workspace;
  readonly boardId: string;
  readonly openCard: { readonly missing: boolean } | undefined;
  status: LoadStatus;
  missing: boolean;
  board: Container | undefined;
  views: BoardView[];
  cards: CardSummary[];
  members: ContainerMember[];
  permissions: ContainerPermissions;
  permissionsRole: ContainerMember['role'] | '' | undefined;
  closeCard(): void;
  isClosed(): boolean;
}

/** Joins an open board and reloads it on success; handles 404 by evicting cache and marking missing. */
export async function joinOpenBoard(
  host: BoardFailureHost & { joining: boolean; load(): Promise<void> },
): Promise<boolean> {
  if (host.joining) return false;
  host.joining = true;
  try {
    await host.workspace.backend.joinContainer(host.boardId);
  } catch (error) {
    if (isBackendError(error) && error.status === 404) {
      host.workspace.evictContainer(host.boardId);
      host.missing = true;
    } else host.workspace.report('save', error);
    return false;
  } finally {
    host.joining = false;
  }
  if (host.isClosed()) return false;
  await host.load();
  return host.status === 'ready';
}

/**
 * Handles a failed board load or refresh, clearing sensitive cached state on
 * 403/404. A 403 is explained by reading the board again; what that finds is
 * not taken once another reading was asked.
 */
export async function handleBoardLoadFailure(
  host: BoardFailureHost,
  error: unknown,
): Promise<void> {
  host.status = 'failed';
  if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
    host.workspace.evictContainer(host.boardId);
    if (!host.openCard?.missing) host.closeCard();
    [host.board, host.views, host.cards, host.members, host.permissions, host.permissionsRole] = [
      undefined,
      [],
      [],
      [],
      { ...READ_ONLY_PERMISSIONS },
      undefined,
    ];
    if (error.status === 404) {
      host.missing = true;
      return;
    }
    const explained = await explainBoardRefusal(host.workspace, host.boardId);
    // A reading asked meanwhile, read or under way, says for itself what the board is.
    if (host.isClosed() || !stillFailed(host)) return;
    if (explained.kind === 'missing') {
      host.missing = true;
      return;
    }
  }
  host.workspace.report('load', error);
}

/** Whether the host's reading failed still, asked after a wait in which another may have been asked. */
function stillFailed(host: BoardFailureHost): boolean {
  return host.status === 'failed';
}

/** Handles a failed board refresh: reports 401, evicts on 403/404, and updates active state. */
export async function handleBoardRefreshFailure(
  host: BoardFailureHost,
  error: unknown,
  isCurrent: () => boolean,
): Promise<void> {
  if (isBackendError(error) && error.status === 401) {
    host.workspace.report('load', error);
    return;
  }
  const accessLost = isBackendError(error) && (error.status === 403 || error.status === 404);
  if (accessLost) host.workspace.evictContainer(host.boardId);
  if (!isCurrent()) return;
  if (accessLost) await handleBoardLoadFailure(host, error);
  else host.workspace.report('load', error);
}

export async function readBoard(workspace: Workspace, boardId: string) {
  const backend = workspace.backend;
  const visit = workspace.info;
  const [board, views, cards, members, permissions] = await Promise.all([
    containerOfWorkspace(backend, boardId, visit?.id),
    backend.views(boardId),
    backend.cards(boardId),
    backend.members(boardId),
    backend.myPermissions(boardId),
  ]);
  if (workspace.info === visit) {
    const memberIds = members.map(memberId);
    const named = [...authorIds(cards), ...assignedIds(cards, board.cardProperties)];
    void workspace.learnUsers([board.createdBy, ...named, ...memberIds], { members: memberIds });
    listOpened(workspace, board);
  }
  return [board, views, cards, members, permissions] as const;
}

/**
 * Classifies a 403 refusal on a board: an open non-template board of the
 * workspace may be joined (`joinable`), a 404 or board of another workspace is
 * `missing`, and anything else is an unexplained refusal (`forbidden`).
 */
export async function explainBoardRefusal(
  workspace: Workspace,
  boardId: string,
): Promise<{ kind: 'joinable'; board: Container } | { kind: 'missing' } | { kind: 'forbidden' }> {
  try {
    const board = await containerOfWorkspace(workspace.backend, boardId, workspace.info?.id);
    if (board.visibility !== 'open' || board.isTemplate) return { kind: 'forbidden' };
    return { kind: 'joinable', board };
  } catch (error) {
    return isBackendError(error) && error.status === 404
      ? { kind: 'missing' }
      : { kind: 'forbidden' };
  }
}
