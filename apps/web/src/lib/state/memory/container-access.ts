/**
 * Content belongs to members: open boards ask others to join; private ones are
 * missing. An open wiki is read by whoever works in the workspace, as a viewer
 * reads it, without joining it: only joined do they write in it. This world has
 * no guests: everybody in it works there.
 */
import type { ContainerKind } from '../../logic/wire.ts';
import { permissionsOfRole, type ContainerPermissions } from '../authority-types.ts';

import { roleOn, type Role } from './comments.ts';
import { fail } from './refuse.ts';
import { workspacesOf, workersOf } from './spaces.ts';
import type { World } from './world.ts';

/**
 * The reader's role on a board or a wiki of the `kind` (the server answers a wiki to an
 * operation of cards as a board that is not there); `any` for one of either.
 */
export function memberRole(
  world: World,
  userId: string,
  containerId: string,
  kind: ContainerKind | 'any' = 'board',
): Exclude<Role, ''> {
  const container = world.containers.get(containerId);
  if (!container || (kind !== 'any' && (container.kind ?? 'board') !== kind))
    fail(404, 'no such board');
  const role = roleOn(world.members.get(containerId) ?? [], userId);
  if (role !== '') return role;
  const works = workersOf(world, container.workspaceId).has(userId);
  if (
    (isOpenToRead(container) || (container.isTemplate && container.visibility === 'open')) &&
    works
  ) {
    return 'viewer';
  }
  if (container.isBuiltin) return 'viewer';
  fail(404, 'no such board');
}

/**
 * Whether the user reads the board, wiki, or course: has a role on it, or works in the
 * workspace of an open non-diary container.
 */
export function reads(world: World, userId: string, containerId: string): boolean {
  const container = world.containers.get(containerId);
  if (!container) return false;
  if (roleOn(world.members.get(containerId) ?? [], userId) !== '') return true;
  if (container.isBuiltin) return true;
  return (
    (isOpenToRead(container) || (container.isTemplate && container.visibility === 'open')) &&
    workersOf(world, container.workspaceId).has(userId)
  );
}

/** Caller-specific `ContainerPermissions` in the memory world (`myPermissions`). */
export function memoryPermissionsOf(
  world: World,
  userId: string,
  containerId: string,
): ContainerPermissions {
  const container = world.containers.get(containerId) ?? fail(404, 'no such board');
  const role = roleOn(world.members.get(containerId) ?? [], userId);
  const wsInfo = workspacesOf(world, userId).find((w) => w.id === container.workspaceId);
  const isWorkspaceAdmin = wsInfo?.role === 'admin';
  const isGuest = wsInfo?.role === 'guest';
  if (role === '' && !isWorkspaceAdmin && !reads(world, userId, containerId)) {
    fail(404, 'no such board');
  }
  return permissionsOfRole(role, {
    isGuest,
    isWorkspaceAdmin,
    isTemplate: container.isTemplate,
  });
}

/** An open board, wiki, or course: read by whoever works in its workspace (`OpenToRead`). */
export function isOpenToRead(container: {
  visibility: string;
  kind?: string;
  isTemplate?: boolean;
}): boolean {
  if (container.visibility !== 'open' || container.isTemplate) return false;
  const kind = container.kind ?? 'board';
  return kind === 'board' || kind === 'wiki' || kind === 'course';
}

/** Alias kept for existing callers: open wikis and courses are read without joining. */
export const isOpenWiki = isOpenToRead;

/** A local blob URL grants direct access, so do not make or return it for a nonmember. */
export function memberFileUrl(
  world: World,
  userId: string,
  containerId: string,
  fileId: string,
): string | undefined {
  if (!reads(world, userId, containerId)) return undefined;
  const file = world.files.get(fileId);
  if (file?.containerId !== containerId || !file.blob) return undefined;
  file.url ??= URL.createObjectURL(file.blob);
  return file.url;
}
