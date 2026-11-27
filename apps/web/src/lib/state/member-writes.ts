/**
 * The members of a board or a wiki as its admins change them: who is added,
 * whose role changes, who is taken off. Each change shows at once; refused, it
 * goes back, is told (`workspace.report`), and the members are read again,
 * unless a list told meanwhile is newer than that reading.
 */
import type { Container, ContainerMember } from '../logic/wire.ts';

import { isBackendError, type ContainerRole } from './backend.ts';
import type { Workspace } from './workspace.svelte.ts';

/** What holds the members of one board or wiki. */
export interface MembersOf {
  readonly workspace: Workspace;
  readonly containerId: string;
  members: ContainerMember[];
}

/**
 * A board or a wiki as the dialogs of its people read and change it
 * (`ShareDialog`, `ContainerTeams`): its openness, the role of who joins it, and
 * its members.
 */
export interface Shareable extends MembersOf {
  readonly container: Container | undefined;
  readonly orderedMembers: ContainerMember[];
  setOpen(open: boolean): Promise<void>;
  setMinimumRole(role: ContainerRole): Promise<void>;
  addMember(userId: string, role: ContainerRole): Promise<void>;
  setMemberRole(userId: string, role: ContainerRole): Promise<void>;
  removeMember(userId: string): Promise<void>;
}

export class MemberWrites {
  readonly #of: MembersOf;
  /** Lists of members told by changes: a reading begun before the latest does not replace it. */
  told = 0;

  constructor(of: MembersOf) {
    this.#of = of;
  }

  /** Makes a person of the workspace a member. Somebody who is a member already keeps their role. */
  async add(userId: string, role: ContainerRole): Promise<void> {
    const of = this.#of;
    try {
      const member = await of.workspace.backend.addMember(of.containerId, userId, role);
      of.members = [...of.members.filter((one) => one.userId !== userId), member];
      void of.workspace.learnUsers([userId], { members: true });
    } catch (error) {
      of.workspace.report('save', error);
    }
  }

  async setRole(userId: string, role: ContainerRole): Promise<void> {
    const of = this.#of;
    await this.#write(
      of.members.map((member) => (member.userId === userId ? { ...member, role } : member)),
      () => of.workspace.backend.setMemberRole(of.containerId, userId, role),
    );
  }

  async remove(userId: string): Promise<void> {
    const of = this.#of;
    await this.#write(
      of.members.filter((member) => member.userId !== userId),
      () => of.workspace.backend.removeMember(of.containerId, userId),
    );
  }

  /**
   * Shows a change of the members at once. Refused, they go back and are read
   * again: what was put back may be older than what arrived meanwhile.
   */
  async #write(optimistic: ContainerMember[], send: () => Promise<unknown>): Promise<void> {
    const of = this.#of;
    const before = of.members;
    of.members = optimistic;
    try {
      await send();
    } catch (error) {
      of.members = before;
      if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
        of.workspace.evictContainer(of.containerId);
      }
      of.workspace.report('save', error);
      const told = this.told;
      try {
        const members = await of.workspace.backend.members(of.containerId);
        // Members told meanwhile are newer than this reading.
        if (told === this.told) of.members = members;
      } catch {
        // The next message, or the next reading, brings them.
      }
    }
  }
}
