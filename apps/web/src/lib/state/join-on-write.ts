/**
 * Joining an open board or wiki by the reader's first write on it. A reader
 * who works in the workspace (no guest) and is no member of an open container
 * whose least role writes or comments sees it as if joined (`joinedPermissions`),
 * and the first write joins first: there is no gate to press before writing.
 * A course is joined by its own button, as what it keeps of the reader starts then.
 */
import type { ContainerMember, MemberRole } from '../logic/container-members.ts';
import type { Container } from '../logic/wire.ts';

import { permissionsOfRole, type ContainerPermissions } from './authority-types.ts';
import { roleOf } from './board-parts.ts';
import type { Workspace } from './workspace.svelte.ts';

/** The role joining gives: the least role, as the server grants it (an editor's where none is set). */
function joinedRole(minimumRole: MemberRole | undefined): MemberRole {
  return minimumRole === 'admin' ? 'editor' : minimumRole || 'editor';
}

/** What the reader may do once joined to a container whose least role is `minimumRole`. */
export function joinedPermissions(
  minimumRole: MemberRole | undefined,
  workspaceAdmin: boolean,
): ContainerPermissions {
  return permissionsOfRole(joinedRole(minimumRole), { isWorkspaceAdmin: workspaceAdmin });
}

/** Whether a first write joins: the container is open and joinable, to one who is no guest, and joined they write. */
export function joinsOnWrite(
  joinable: boolean,
  workspace: Workspace,
  minimumRole: MemberRole | undefined,
): boolean {
  if (!joinable || workspace.info?.role === 'guest') return false;
  const joined = joinedPermissions(minimumRole, false);
  return joined.canEdit || joined.canComment;
}

export interface JoinOnWriteHost {
  readonly workspace: Workspace;
  /** Whether the next write joins first (`joinsOnWrite`). */
  readonly joinsOnWrite: boolean;
  members: ContainerMember[];
}

/**
 * Joins before a write, once for every write that waits on it; a write that
 * joins nothing goes at once, as it would without this. A join that is
 * refused leaves the write to go and be refused as well, which says so as any
 * refused write does.
 */
export class JoinOnWrite {
  readonly #host: JoinOnWriteHost;
  /** Reads again what the reader may do, now a member of `role`: what is read is theirs as one. */
  readonly #refreshPermissions: (role: MemberRole) => Promise<void>;
  #joining: Promise<void> | undefined;

  constructor(host: JoinOnWriteHost, refreshPermissions: (role: MemberRole) => Promise<void>) {
    this.#host = host;
    this.#refreshPermissions = refreshPermissions;
  }

  /** Sends `write`: at once, or where it is the reader's first on `containerId` once they are a member. */
  run<T>(containerId: string, write: () => Promise<T>): Promise<T> {
    if (this.#host.joinsOnWrite) {
      this.#joining ??= this.#join(containerId).finally(() => {
        this.#joining = undefined;
      });
    }
    return this.#joining ? this.#joining.then(write) : write();
  }

  async #join(containerId: string): Promise<void> {
    const host = this.#host;
    let member: ContainerMember;
    try {
      member = await host.workspace.backend.joinContainer(containerId);
    } catch {
      return;
    }
    // What a member may do is read before the reader is one, so that nothing reads as before in
    // between; it is told as the joined member's role's, which the reader has from then on.
    await this.#refreshPermissions(roleOf(member));
    host.members = [...host.members.filter((one) => one.userId !== member.userId), member];
  }
}

/**
 * Lists an open board or wiki the reader has opened among the workspace's, where
 * the list was read before it was made: it then waits, with the rest the reader
 * may join, at the sidebar's foot.
 */
export function listOpened(workspace: Workspace, container: Container): void {
  if (container.visibility !== 'open') return;
  if (workspace.containers.some((known) => known.id === container.id)) return;
  workspace.receiveContainer(container);
}
