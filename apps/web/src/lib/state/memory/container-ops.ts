/**
 * Container, membership, public-link, and archive operations for `MemoryBackend`
 * (`state/memory/backend.ts`), extracted to keep `MemoryBackend` within 500 lines
 * without splitting or replacing the `MemoryBackend` class.
 */
import {
  createBoard,
  type BoardView,
  type Container,
  type ContainerKind,
  type ContainerMember,
  type ContainerPatch,
} from '../../logic/wire.ts';
import type {
  BoardDraft,
  ContainerRole,
  CopiedContainerOutput,
  CopyToWorkspaceInput,
  DownloadedFile,
  DownloadOptions,
  ImportOptions,
  ImportSummary,
  PublicLink,
  PublicLinkChange,
  ViewDraft,
} from '../backend.ts';
import { applyContainerPatch } from '../patches.ts';

import { settingsOf } from './account.ts';
import { archiveOf, exportableContainers, readImportFile, summarizeImport } from './archive.ts';
import { checkArchiveCards } from './card-hierarchy.ts';
import { fileAsNew } from './categories.ts';
import { memberRole } from './container-access.ts';
import { copiedBlocks, copyTitle, insertCopy } from './copies.ts';
import { purgeMemberExamScores } from './course-exams.ts';
import { checkDiariesComing } from './diary-places.ts';
import { fail } from './refuse.ts';
import { welcomeTemplate } from './seed-templates.ts';
import { copy, inPropertyOrder, memberOf, viewOf, wireOf } from './shapes.ts';
import * as spaces from './spaces.ts';
import { deletion, placesOf, takeContainerBack } from './trash.ts';
import type { StoredBlock, World } from './world.ts';

export interface MemoryContainerContext {
  readonly world: World;
  readonly userId: string;
}

export function containerInWorld(
  world: World,
  containerId: string,
  kind: ContainerKind | 'any' | 'shared' = 'any',
): Container {
  const board = world.containers.get(containerId);
  const found = board?.kind ?? 'board';
  const ok = kind === 'any' || (kind === 'shared' && found !== 'diary') || found === kind;
  return board && ok ? board : fail(404, 'no board');
}

export function memberIdsOf(world: World, containerId: string): string[] {
  return (world.members.get(containerId) ?? []).map((member) => member.userId);
}

export function tellContainer(world: World, board: Container): void {
  world.tellContainer(board.id, { kind: 'container', container: copy(board) });
  world.tellUsers(memberIdsOf(world, board.id), { kind: 'container', container: copy(board) });
}

export function tellBlocks(world: World, id: string, blocks: readonly StoredBlock[]): void {
  world.tellContainer(id, { kind: 'blocks', containerId: id, blocks: blocks.map(wireOf) });
}

export function newStoredBlock(
  ctx: MemoryContainerContext,
  boardId: string,
  parentId: string,
  type: StoredBlock['type'],
): StoredBlock {
  const now = ctx.world.now();
  return {
    id: ctx.world.newId(),
    containerId: boardId,
    parentId,
    createdBy: ctx.userId,
    modifiedBy: ctx.userId,
    type,
    title: '',
    fields: {},
    createdAt: now,
    updatedAt: now,
  };
}

export function createMemoryBoard(
  ctx: MemoryContainerContext,
  workspaceId: string,
  draft: BoardDraft,
  firstView: ViewDraft,
): { board: Container; view: BoardView } {
  const now = ctx.world.now();
  const board: Container = {
    ...createBoard(),
    id: ctx.world.newId(),
    workspaceId,
    createdBy: ctx.userId,
    modifiedBy: ctx.userId,
    visibility: draft.visibility ?? 'private',
    minimumRole: 'editor',
    title: draft.title,
    description: draft.description ?? '',
    icon: draft.icon ?? '',
    createdAt: now,
    updatedAt: now,
  };
  if (draft.scaffold && !draft.cardProperties) {
    const statusId = ctx.world.newId();
    const assigneeId = ctx.world.newId();
    const dueId = ctx.world.newId();
    const todoOpt = ctx.world.newId();
    const doingOpt = ctx.world.newId();
    const doneOpt = ctx.world.newId();
    board.cardProperties = [
      {
        id: statusId,
        name: 'Status',
        type: 'select',
        options: [
          { id: todoOpt, label: 'To do', color: 'gray' },
          { id: doingOpt, label: 'In progress', color: 'blue' },
          { id: doneOpt, label: 'Done', color: 'green' },
        ],
      },
      { id: assigneeId, name: 'Assignee', type: 'person', options: [] },
      { id: dueId, name: 'Due Date', type: 'date', options: [] },
    ];
    board.dueDatePropertyId = dueId;
    board.doneWhen = { kind: 'option', propertyId: statusId, optionIds: [doneOpt] };
  } else if (draft.cardProperties) {
    board.cardProperties = copy(draft.cardProperties);
  }
  ctx.world.containers.set(board.id, board);
  ctx.world.members.set(board.id, [memberOf(board.id, ctx.userId, 'admin')]);
  fileAsNew(ctx.world, ctx.userId, board);
  const view = newStoredBlock(ctx, board.id, '', 'view');
  view.title = firstView.title;
  const statusProp = board.cardProperties.find((prop) => prop.type === 'select');
  view.fields =
    draft.scaffold && statusProp
      ? {
          ...copy(firstView.fields),
          viewType: 'board',
          groupBy: statusProp.id,
          visibleGroups: [...statusProp.options.map((opt) => opt.id), ''],
          shownProperties: board.cardProperties
            .filter((prop) => prop.id !== statusProp.id)
            .map((prop) => prop.id),
          order: 'a0',
        }
      : copy(firstView.fields);
  ctx.world.blocks.set(view.id, view);
  tellContainer(ctx.world, board);
  tellBlocks(ctx.world, board.id, [view]);
  return { board: copy(board), view: viewOf(view) };
}

export function patchMemoryContainer(
  ctx: MemoryContainerContext,
  id: string,
  patch: ContainerPatch,
): Container {
  const board = applyContainerPatch(containerInWorld(ctx.world, id), patch);
  ctx.world.containers.set(id, board);
  board.updatedAt = ctx.world.now();
  board.modifiedBy = ctx.userId;
  tellContainer(ctx.world, board);
  return copy(board);
}

export function reorderMemoryCardProperties(
  ctx: MemoryContainerContext,
  boardId: string,
  propertyIds: string[],
): string[] {
  const current = containerInWorld(ctx.world, boardId, 'board');
  if (!['admin', 'editor'].includes(memberRole(ctx.world, ctx.userId, boardId))) {
    fail(403, 'access denied to modifying board properties');
  }
  const board = inPropertyOrder(current, propertyIds);
  board.updatedAt = ctx.world.now();
  board.modifiedBy = ctx.userId;
  ctx.world.containers.set(boardId, board);
  tellContainer(ctx.world, board);
  return [...propertyIds];
}

export function deleteMemoryContainer(ctx: MemoryContainerContext, id: string): void {
  const container = containerInWorld(ctx.world, id);
  const members = ctx.world.members.get(id) ?? [];
  const blocks = ctx.world.blocksOf(id);
  const places = placesOf(ctx.world, id);
  ctx.world.deletedContainers.set(id, { container, members, blocks, places, ...deletion(ctx) });
  for (const block of blocks) ctx.world.blocks.delete(block.id);
  ctx.world.containers.delete(id);
  ctx.world.members.delete(id);
  for (const categories of ctx.world.categories.values()) {
    for (const category of categories) {
      category.containerIds = category.containerIds.filter((other) => other !== id);
      category.hiddenContainerIds = category.hiddenContainerIds.filter((other) => other !== id);
    }
  }
  const userIds = members.map((member) => member.userId);
  ctx.world.tellContainer(id, { kind: 'containerDeleted', containerId: id });
  ctx.world.tellUsers(userIds, { kind: 'containerDeleted', containerId: id });
}

export function restoreMemoryContainer(ctx: MemoryContainerContext, id: string): Container {
  const board = takeContainerBack(ctx.world, ctx.userId, id);
  tellContainer(ctx.world, board);
  return copy(board);
}

function insertMemoryContainer(
  ctx: MemoryContainerContext,
  source: Container,
  blocks: readonly StoredBlock[],
  workspaceId: string,
  schedule?: { startDate: string; targetDate: string; studyDays: number },
): Container {
  const board = insertCopy(ctx.world, ctx.userId, { source, blocks, workspaceId, schedule });
  tellContainer(ctx.world, board);
  return copy(board);
}

export function duplicateMemoryContainer(
  ctx: MemoryContainerContext,
  id: string,
  asTemplate: boolean,
  to: string,
): Container {
  const source = containerInWorld(ctx.world, id);
  if (source.kind === 'diary') fail(404, 'no board');
  if (source.kind === 'wiki' && asTemplate) fail(400, 'a wiki is no template');
  if (!source.isTemplate && !memberIdsOf(ctx.world, id).includes(ctx.userId)) {
    fail(403, 'access denied to copy board');
  }
  if (source.isBuiltin !== true && source.workspaceId !== to) fail(404, 'no board');
  const title = copyTitle(source, asTemplate, settingsOf(ctx.world, ctx.userId).language);
  const blocks = copiedBlocks(ctx.world, source);
  return insertMemoryContainer(ctx, { ...source, isTemplate: asTemplate, title }, blocks, to);
}

export function copyMemoryContainerToWorkspace(
  ctx: MemoryContainerContext,
  id: string,
  request: CopyToWorkspaceInput,
): CopiedContainerOutput {
  const source = containerInWorld(ctx.world, id);
  if (source.kind === 'diary') fail(404, 'no board');
  spaces.checkWorkspace(ctx.world, ctx.userId, request.targetWorkspaceId);
  if (!memberIdsOf(ctx.world, id).includes(ctx.userId)) {
    fail(403, 'access denied to copy board');
  }
  if (source.workspaceId !== request.targetWorkspaceId && !ctx.world.allowCopyOut) {
    fail(403, 'copying out of this organization is disabled', { reason: 'copy_out_disabled' });
  }
  const title =
    request.title?.trim() || copyTitle(source, false, settingsOf(ctx.world, ctx.userId).language);
  const blocks = copiedBlocks(ctx.world, source);
  const container = insertMemoryContainer(
    ctx,
    { ...source, isTemplate: false, visibility: 'private', title },
    blocks,
    request.targetWorkspaceId,
  );
  return { container, missingFilesCount: 0 };
}

export function onboardMemoryWorkspace(ctx: MemoryContainerContext, workspaceId: string): string {
  const welcome = welcomeTemplate(ctx.world, ctx.userId) ?? fail(404, 'no welcome board');
  const blocks = copiedBlocks(ctx.world, welcome);
  return insertMemoryContainer(ctx, { ...welcome, isTemplate: false }, blocks, workspaceId).id;
}

export function exportMemoryContainer(
  ctx: MemoryContainerContext,
  id: string,
  options?: DownloadOptions,
): DownloadedFile {
  const board = containerInWorld(ctx.world, id);
  if (!memberIdsOf(ctx.world, id).includes(ctx.userId)) fail(403, 'not a member of the board');
  return archiveOf(ctx.world, [board], options);
}

export function exportMemoryWorkspace(
  ctx: MemoryContainerContext,
  workspaceId: string,
  options?: DownloadOptions,
): DownloadedFile {
  spaces.checkWorkspace(ctx.world, ctx.userId, workspaceId);
  return archiveOf(ctx.world, exportableContainers(ctx.world, workspaceId, ctx.userId), options);
}

export async function importMemoryArchive(
  ctx: MemoryContainerContext,
  workspaceId: string,
  file: File,
  options?: ImportOptions,
): Promise<ImportSummary> {
  spaces.checkWorkspace(ctx.world, ctx.userId, workspaceId);
  const format = options?.format ?? 'fvoci';
  const { containers, blocks, schedules, skipped } = await readImportFile(
    file,
    format,
    workspaceId,
  );
  options?.onProgress?.({ sentBytes: file.size, totalBytes: file.size });
  checkDiariesComing(ctx.world, ctx.userId, workspaceId, { containers, blocks });
  checkArchiveCards(containers, blocks);
  const insertedBlocks: StoredBlock[] = [];
  const created = containers.map((board) => {
    const own = blocks.filter((b) => b.containerId === board.id);
    insertedBlocks.push(...own);
    return insertMemoryContainer(ctx, board, own, workspaceId, schedules.get(board.id));
  });
  return summarizeImport(format, created, insertedBlocks, skipped);
}

function setMembers(world: World, id: string, members: ContainerMember[]): void {
  world.members.set(id, members);
  world.tellContainer(id, { kind: 'members', containerId: id, members: copy(members) });
}

function keepAnAdmin(members: readonly ContainerMember[]): void {
  if (!members.some((member) => !member.synthetic && member.role === 'admin')) {
    fail(409, 'a board keeps its last admin', { reason: 'last_admin' });
  }
}

export function joinMemoryContainer(ctx: MemoryContainerContext, id: string): ContainerMember {
  const board = containerInWorld(ctx.world, id);
  if (board.isTemplate) fail(403, 'a template cannot be joined');
  const works = spaces.workersOf(ctx.world, board.workspaceId).has(ctx.userId);
  if (board.visibility !== 'open' || !works) fail(404, 'no such open board');
  const members = ctx.world.members.get(id) ?? [];
  const existing = members.find((member) => member.userId === ctx.userId);
  if (existing) return copy(existing);
  const role = board.minimumRole === 'admin' ? 'editor' : board.minimumRole || 'editor';
  const member = memberOf(id, ctx.userId, role);
  setMembers(ctx.world, id, [...members, member]);
  // Filed under the reader's default category, as the server files what is joined.
  fileAsNew(ctx.world, ctx.userId, board);
  return copy(member);
}

export function addMemoryMember(
  ctx: MemoryContainerContext,
  id: string,
  userId: string,
  role: ContainerRole,
): ContainerMember {
  const into = containerInWorld(ctx.world, id, 'shared').workspaceId;
  if (!spaces.workersOf(ctx.world, into).has(userId)) {
    fail(400, 'no such user', { field: 'userId' });
  }
  const members = ctx.world.members.get(id) ?? [];
  const existing = members.find((member) => member.userId === userId);
  if (existing) return copy(existing);
  const member = memberOf(id, userId, role);
  setMembers(ctx.world, id, [...members, member]);
  fileAsNew(ctx.world, userId, containerInWorld(ctx.world, id));
  ctx.world.tellUsers([userId], {
    kind: 'container',
    container: copy(containerInWorld(ctx.world, id)),
  });
  return copy(member);
}

export function setMemoryMemberRole(
  ctx: MemoryContainerContext,
  id: string,
  userId: string,
  role: ContainerRole,
): ContainerMember {
  containerInWorld(ctx.world, id, 'shared');
  const members = ctx.world.members.get(id) ?? [];
  if (!members.some((member) => member.userId === userId)) fail(404, 'no such member');
  const member = memberOf(id, userId, role);
  const next = members.map((one) => (one.userId === userId ? member : one));
  keepAnAdmin(next);
  setMembers(ctx.world, id, next);
  return copy(member);
}

export function removeMemoryMember(ctx: MemoryContainerContext, id: string, userId: string): void {
  containerInWorld(ctx.world, id, 'shared');
  const members = ctx.world.members.get(id) ?? [];
  const next = members.filter((member) => member.userId !== userId);
  if (next.length === members.length) return;
  keepAnAdmin(next);
  purgeMemberExamScores(ctx.world, id, userId);
  setMembers(ctx.world, id, next);
  ctx.world.tellGone(userId, id);
}

export function setMemoryPublicLink(
  ctx: MemoryContainerContext,
  id: string,
  change: PublicLinkChange,
): PublicLink {
  containerInWorld(ctx.world, id, 'shared');
  const before = ctx.world.publicLinks.get(id);
  const token =
    !before?.token || change.regenerate ? ctx.world.newId().replaceAll('-', '') : before.token;
  const link = { enabled: change.enabled, token };
  ctx.world.publicLinks.set(id, link);
  return copy(link);
}
