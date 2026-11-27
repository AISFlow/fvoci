/**
 * Of the open containers no category of the reader's files, the ones the
 * reader is known to be a member of: directly, or by a group (a membership
 * with an `inheritedRole`), as read where each was opened. The server files a
 * container in the reader's categories when they join it or are added to it,
 * not when a group of theirs is given a role on it: such a one is the
 * reader's all the same, listed with their own (`sidebarOf`), not among those
 * that wait to be joined (`openToJoinOf`).
 */
import { SvelteMap } from 'svelte/reactivity';

import type { ContainerMember } from '../logic/container-members.ts';

import { roleOf } from './board-parts.ts';
import type { SidebarReader } from './order.ts';
import type { Workspace } from './workspace.svelte.ts';

/**
 * What is known of each workspace (one for the app), each change a new set. Made as the module
 * loads, outside anything that reads it: state made while a derived runs is not a dependency of
 * that run, and the sidebar that made it would miss the first change.
 */
const known = new SvelteMap<Workspace, ReadonlySet<string>>();
const NONE: ReadonlySet<string> = new Set();

/** Notes, from the members of `containerId` as read, whether the reader is one of them. */
export function noteMembership(
  workspace: Workspace,
  containerId: string,
  members: readonly ContainerMember[],
): void {
  const meId = workspace.me?.id;
  const member = members.some((one) => one.userId === meId && roleOf(one) !== '');
  const before = known.get(workspace) ?? NONE;
  if (member === before.has(containerId)) return;
  known.set(
    workspace,
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a snapshot never changed: a change is a new one in `known`
    new Set(member ? [...before, containerId] : [...before].filter((one) => one !== containerId)),
  );
}

/** The reader of `workspace`'s sidebar: who they are, whether a guest, and what they are known to be a member of. */
export function readerOf(workspace: Workspace): SidebarReader {
  return {
    id: workspace.me?.id,
    guest: workspace.info?.role === 'guest',
    memberOf: known.get(workspace) ?? NONE,
  };
}
