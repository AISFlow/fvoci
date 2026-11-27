/**
 * The guest links of a shared board in its share dialog: the ones mailed to
 * somebody by their address, each to be revoked, against a backend that answers
 * as the server would.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import GuestInvite from '$lib/features/shell/GuestInvite.svelte';
import { BackendError, Workspace, type Invitation, type People } from '$lib/state/index.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';

afterEach(cleanup);

function link(id: string, extra: Partial<Invitation> = {}): Invitation {
  return {
    id,
    role: 'guest',
    containerId: 'b1',
    createdBy: 'u1',
    createdAt: 0,
    expiresAt: 7 * 86_400_000,
    uses: 0,
    ...extra,
  };
}

const LINKS: Invitation[] = [
  link('parent'),
  link('child', { parentId: 'parent', recipient: 'guest@example.test' }),
  link('other-board', { containerId: 'b2', recipient: 'elsewhere@example.test' }),
  { ...link('members'), role: 'member' },
];

function mount(read: People['invitations']) {
  const revokeInvitation = vi.fn<People['revokeInvitation']>(() => Promise.resolve());
  const invitations = vi.fn(read);
  const people: People = {
    members: () => Promise.resolve([]),
    setRole: () => Promise.reject(new Error('unused')),
    removeMember: () => Promise.resolve(),
    invitations,
    revokeInvitation,
    inviteMembers: () => Promise.reject(new Error('unused')),
    inviteGuest: vi.fn<People['inviteGuest']>(() => Promise.reject(new Error('not here'))),
    guestInvitations: () => Promise.resolve(true),
    setGuestInvitations: () => Promise.resolve(true),
  };
  const { world, readerId } = seedWorld([]);
  const workspace = new Workspace({
    backend: new MemoryBackend(world, readerId),
    locale: () => 'en',
  });
  workspace.status = 'ready';
  workspace.workspaces = [{ id: 'w1', title: 'Acme', role: 'admin' }];
  workspace.info = { id: 'w1', title: 'Acme', role: 'admin' };
  workspace.settings = { ...workspace.settings, timeZone: 'UTC', hourCycle: 'h23' };
  render(GuestInvite, {
    props: {
      workspace,
      access: { people, inviteLink: (token: string) => token },
      containerId: 'b1',
    },
  });
  return { invitations, revokeInvitation, workspace };
}

describe('the guest links of a shared board', () => {
  it('lists the board’s guest links, the mailed ones by their address, and revokes one', async () => {
    const { revokeInvitation } = mount(() => Promise.resolve(LINKS));
    const list = await screen.findByTestId('share-guest-links');
    const rows = within(list).getAllByRole('listitem');
    expect(rows.map((row) => row.textContent.replace(/\s+/g, ' ').trim())).toEqual([
      'Link · ends January 08, 1970 Revoke link',
      'Mailed to guest@example.test · ends January 08, 1970 Revoke link',
    ]);
    const mailed = rows[1];
    if (!mailed) throw new Error('two rows');
    await fireEvent.click(within(mailed).getByRole('button', { name: 'Revoke link' }));
    await waitFor(() => {
      expect(revokeInvitation).toHaveBeenCalledWith('w1', 'child');
    });
  });

  it('leaves the list out when it cannot be read', async () => {
    const { invitations } = mount(() => Promise.reject(new BackendError(403, { detail: 'no' })));
    await waitFor(() => {
      expect(invitations).toHaveBeenCalled();
    });
    expect(screen.queryByTestId('share-guest-links')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
  });
});
