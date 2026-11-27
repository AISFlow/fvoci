/**
 * Where a board, a wiki or a course may be copied to, and the organizations
 * known to keep theirs in: those whose administrators switched copying out
 * off (`allowCopyOut`). Only an administrator reads that switch (`OrgAdmin`),
 * and what they read holds until it is read again. Anybody else learns it from
 * the refusal of a copy (`copy_out_disabled`), which holds while the dialog
 * that met it stays open: opened again, it tries again, as the switch may
 * have been turned back on meanwhile.
 */
import { SvelteMap } from 'svelte/reactivity';

import { isOpen } from './workspace-entry.ts';
import type { WorkspaceInfo } from './workspace-types.ts';

/** How it was learnt that the place keeps its containers in. */
type Learnt = 'read' | 'refused';

const keptIn = new SvelteMap<string, Learnt>();

/**
 * What a workspace's containers are copied out of: its organization, or the
 * workspace itself on a backend without organizations.
 */
export function copyOutPlace(workspace: Pick<WorkspaceInfo, 'id' | 'orgId'>): string {
  return workspace.orgId ?? workspace.id;
}

/**
 * The workspaces a container of `sourceId` may be copied into: every other
 * one the session opens, but those the reader is a guest of, who makes
 * nothing there (the server answers 404).
 */
export function copyPlaces(
  workspaces: readonly WorkspaceInfo[],
  sourceId: string | undefined,
): WorkspaceInfo[] {
  return workspaces.filter((ws) => isOpen(ws) && ws.id !== sourceId && ws.role !== 'guest');
}

/** Whether the place is known to refuse copies out of it. */
export function keepsCopiesIn(place: string): boolean {
  return keptIn.has(place);
}

/**
 * What was learnt of the place: whether it lets its containers be copied
 * out, as read from its settings (the default) or from a copy it `refused`.
 * A refusal does not take the place of what was read.
 */
export function noteCopyOut(place: string, allowed: boolean, learnt: Learnt = 'read'): void {
  if (allowed) keptIn.delete(place);
  else if (learnt === 'read' || keptIn.get(place) !== 'read') keptIn.set(place, learnt);
}

/** Forgets what a refusal taught of the place, to try again; what was read of it stays. */
export function forgetCopyRefusal(place: string): void {
  if (keptIn.get(place) === 'refused') keptIn.delete(place);
}
