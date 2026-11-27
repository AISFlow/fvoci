/**
 * The workspaces of the world in memory, as the server has them: the one its
 * people share (`World.workspace`), and every user's own space, made the
 * first time it is asked for. The own space is a workspace like another to
 * what is under it, with its owner alone in it: of anybody else's it is not
 * there (404), and nothing of it is told to anybody else.
 */
import type { UserOrganization, WorkspaceInfo } from '../workspace-types.ts';

import { BackendError } from '../problems.ts';

import { fail } from './refuse.ts';
import { copy } from './shapes.ts';
import type { World } from './world.ts';

/** The user's own space; made, with its default category, the first time it is asked for. */
export function ownSpaceOf(world: World, userId: string): WorkspaceInfo {
  const kept = world.ownSpaces.get(userId);
  if (kept) return kept;
  const own: WorkspaceInfo = { id: world.newId(), title: '', kind: 'personal' };
  world.ownSpaces.set(userId, own);
  const now = world.now();
  world.categories.set(userId, [
    ...(world.categories.get(userId) ?? []),
    {
      id: world.newId(),
      name: 'Boards',
      userId,
      workspaceId: own.id,
      type: 'system',
      collapsed: false,
      sortOrder: 0,
      containerIds: [],
      hiddenContainerIds: [],
      createdAt: now,
      updatedAt: now,
    },
  ]);
  return own;
}

/** The user's workspaces, as the server lists them: their own space last. */
export function workspacesOf(world: World, userId: string): WorkspaceInfo[] {
  return [copy(world.workspace), copy(ownSpaceOf(world, userId))];
}

/** The shared organizations the user is a member of (`GET /users/me/organizations`). */
export function organizationsOf(world: World, userId: string): UserOrganization[] {
  const explicit = world.organizations.get(userId);
  if (explicit !== undefined) return explicit.map(copy);
  const { orgId, kind, title, role } = world.workspace;
  if (orgId && kind !== 'personal' && role !== 'guest' && world.workspaceMembers.has(userId)) {
    return [{ id: orgId, name: title, role: 'member' }];
  }
  return [];
}

/** Who works in the workspace: its owner alone in an own space; nobody in one that is not there. */
export function workersOf(world: World, workspaceId: string): ReadonlySet<string> {
  if (workspaceId === world.workspace.id) return world.workspaceMembers;
  for (const [owner, own] of world.ownSpaces) if (own.id === workspaceId) return new Set([owner]);
  return new Set();
}

/**
 * Gives the user a password and closes their own space to the session, as
 * the server does to a session made through a provider: it opens again to
 * the password (`confirmPassword`).
 */
export function closeOwnSpace(world: World, userId: string, password: string): void {
  world.passwords.set(userId, password);
  ownSpaceOf(world, userId).passwordRequired = true;
}

/**
 * The session gives the account's password, as the server reads it
 * (`confirmPassword`): the own space opens to it. 409 for an account that
 * has none, 400 `invalid_credentials` at the member for another password.
 */
export function confirmPassword(world: World, userId: string, password: string): void {
  const kept = world.passwords.get(userId);
  if (kept === undefined) fail(409, 'the account has no password');
  if (kept !== password) {
    const refusal = { detail: 'the password is wrong', reason: 'invalid_credentials' };
    throw new BackendError(400, { ...refusal, errors: [{ ...refusal, pointer: '/password' }] });
  }
  ownSpaceOf(world, userId).passwordRequired = false;
}

/**
 * 404 unless the user works in the workspace: the shared one, or their own
 * space; 401 `password_sign_in_required` in an own space marked closed to the
 * session (`passwordRequired`), as the server answers a provider's session.
 */
export function checkWorkspace(world: World, userId: string, workspaceId: string): void {
  const own = ownSpaceOf(world, userId);
  if (workspaceId !== world.workspace.id && workspaceId !== own.id) {
    fail(404, 'no such workspace');
  }
  if (workspaceId === own.id && own.passwordRequired === true) {
    throw new BackendError(401, {
      reason: 'password_sign_in_required',
      detail: 'the own space opens to a session made with the password',
    });
  }
}
