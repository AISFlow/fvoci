/**
 * What the reader is in the workspace shown, and managing who is in it where
 * the backend has that (the server; the one in memory has no people to manage).
 * Kept beside the workspace store, which the screens are handed: the app says
 * once what its backend offers (`providePeople`), and a screen asks (`peopleOf`).
 */
import type { People } from './people-types.ts';
import type { Workspace } from './workspace.svelte.ts';

/** What a screen needs to manage people and to invite. */
export interface PeopleAccess {
  people: People;
  /** The whole address of the page of an invitation, by its token. */
  inviteLink: (token: string) => string;
}

const offered = new WeakMap<Workspace, PeopleAccess>();

/** Says what the backend of a workspace store offers; called once, where the store is made. */
export function providePeople(workspace: Workspace, access: PeopleAccess): void {
  offered.set(workspace, access);
}

/** Managing people for the workspace store; undefined where the backend has none of it. */
export function peopleOf(workspace: Workspace): PeopleAccess | undefined {
  return offered.get(workspace);
}

/**
 * Whether the reader is a guest of the workspace shown: on the boards they
 * were invited to, making none, and seeing nothing else of it.
 */
export function isGuest(workspace: Workspace): boolean {
  return workspace.info?.role === 'guest';
}

/** Whether the reader administers the workspace shown: its members and their invitations. */
export function administers(workspace: Workspace): boolean {
  return workspace.info?.role === 'admin';
}
