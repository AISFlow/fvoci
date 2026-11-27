/**
 * A board or a wiki for the invited only that the reader manages without
 * reading it: an administrator of the workspace sees it in the list, manages
 * its members and deletes it, and reads what is in it only as its member,
 * which the workspace's members then read in its records. Its content is not
 * there for the reader (404) while it and its members are.
 */
import type { Container, ContainerKind } from '../logic/wire.ts';

import { isBackendError } from './backend.ts';
import { containerOfWorkspace } from './board-parts.ts';
import type { Shareable } from './member-writes.ts';
import { memberId } from './user-names.ts';
import type { LoadStatus } from './workspace.svelte.ts';

/** The store of a board or a wiki that failed to open as missing. */
export interface Managed extends Shareable {
  container: Container | undefined;
  readonly status: LoadStatus;
  /** Reads it again, as its member now. */
  load(): Promise<void>;
}

/**
 * Whether the board or wiki of the `kind` the store failed to open as missing
 * is one the reader manages without reading. Then the store is given it and
 * its members, for the members to be managed and the reader to join it; it
 * stays failed.
 */
export async function readManagedOnly(
  store: Managed,
  kind: ContainerKind = 'board',
): Promise<boolean> {
  const backend = store.workspace.backend;
  try {
    const [container, members] = await Promise.all([
      containerOfWorkspace(backend, store.containerId, store.workspace.info?.id, kind),
      backend.members(store.containerId),
    ]);
    if (container.visibility !== 'private' || container.isTemplate) return false;
    store.container = container;
    store.members = members;
    void store.workspace.learnUsers(members.map(memberId), { members: true });
    return true;
  } catch (error) {
    if (isBackendError(error) && (error.status === 404 || error.status === 403)) return false;
    throw error;
  }
}

/**
 * Makes the reader a viewer of what they manage, and opens it. False when that
 * failed, or when it is no longer shown (`shown`) by then: it is not read for
 * nobody.
 */
export async function joinManaged(store: Managed, shown: () => boolean): Promise<boolean> {
  const me = store.workspace.me;
  if (!me) return false;
  try {
    await store.workspace.backend.addMember(store.containerId, me.id, 'viewer');
  } catch (error) {
    store.workspace.report('save', error);
    return false;
  }
  if (!shown()) return false;
  await store.load();
  return store.status === 'ready';
}
