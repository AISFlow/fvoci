/** A story-only People implementation; the application memory backend has no People service. */
import { emptyName } from '$lib/logic/person-names.ts';
import {
  BackendError,
  type People,
  type Invitation,
  type WorkspaceMember,
  type WorkspaceRole,
} from '$lib/state/index.ts';

import { STORY_NOW } from './fixtures.ts';

export type PeopleMode = 'ready' | 'loading' | 'load-error' | 'write-error' | 'forbidden';
export const exampleInviteLink = (token: string): string => `https://example.test/invite/${token}`;

export class ExamplePeople implements People {
  #members: WorkspaceMember[];
  #links: Invitation[];
  #guests = true;
  #serial = 0;
  readonly #mode: PeopleMode;

  constructor(count: number, mode: PeopleMode) {
    this.#mode = mode;
    this.#members = Array.from({ length: count }, (_, index) => ({
      userId: index === 0 ? 'example-reader' : `example-member-${index}`,
      username: `example-member-${index}`,
      displayName: `Example member ${index} · 이름이 긴 예시 구성원`,
      // The second has a title and credentials: the list writes them with the name.
      ...(index === 1
        ? { name: { ...emptyName(), title: 'prof' as const, credentials: ['phd' as const] } }
        : {}),
      role: index === 0 ? 'admin' : index % 3 === 0 ? 'guest' : 'member',
    }));
    const guest = this.link('guest');
    this.#links =
      count === 0
        ? []
        : [
            this.link('member'),
            guest,
            // Its own identifier, so the links the stories make keep their numbers.
            {
              ...guest,
              id: 'example-invitation-mailed',
              parentId: guest.id,
              recipient: 'guest@example.test',
            },
          ];
  }

  private async request(write = false): Promise<void> {
    if (this.#mode === 'loading') await new Promise<void>(() => undefined);
    if (this.#mode === 'forbidden')
      throw new BackendError(403, { detail: 'Example permission refusal' });
    if (this.#mode === 'load-error' || (write && this.#mode === 'write-error')) {
      throw new BackendError(503, { detail: 'Example service unavailable' });
    }
  }

  private link(role: Invitation['role']): Invitation {
    const id = `example-invitation-${++this.#serial}`;
    return {
      id,
      role,
      ...(role === 'guest' ? { containerId: 'example-board' } : {}),
      createdBy: 'example-reader',
      createdAt: STORY_NOW,
      expiresAt: STORY_NOW + 7 * 86_400_000,
      uses: 0,
    };
  }

  async members(): Promise<WorkspaceMember[]> {
    await this.request();
    return structuredClone(this.#members);
  }
  async invitations(): Promise<Invitation[]> {
    await this.request();
    return structuredClone(this.#links);
  }
  async guestInvitations(): Promise<boolean> {
    await this.request();
    return this.#guests;
  }
  async setGuestInvitations(_workspaceId: string, enabled: boolean): Promise<boolean> {
    await this.request(true);
    this.#guests = enabled;
    return enabled;
  }
  async setRole(
    _workspaceId: string,
    userId: string,
    role: WorkspaceRole,
  ): Promise<WorkspaceMember> {
    await this.request(true);
    const member = this.#members.find((one) => one.userId === userId);
    if (!member) throw new BackendError(404, { detail: 'Example member missing' });
    member.role = role;
    return { ...member };
  }
  async removeMember(_workspaceId: string, userId: string): Promise<void> {
    await this.request(true);
    this.#members = this.#members.filter((one) => one.userId !== userId);
  }
  async revokeInvitation(_workspaceId: string, id: string): Promise<void> {
    await this.request(true);
    this.#links = this.#links.filter((one) => one.id !== id);
  }
  async inviteMembers(): Promise<Invitation> {
    await this.request(true);
    const link = this.link('member');
    this.#links.unshift(link);
    return { ...link, token: link.id };
  }
  async inviteGuest(): Promise<Invitation> {
    await this.request(true);
    if (!this.#guests)
      throw new BackendError(403, { detail: 'Example guest invitations disabled' });
    const link = this.link('guest');
    this.#links.unshift(link);
    return { ...link, token: link.id };
  }
}
