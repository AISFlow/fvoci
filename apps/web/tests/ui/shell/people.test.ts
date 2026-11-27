/**
 * The people of a workspace in their dialog: what its administrator sees and
 * does there, against a backend that answers as the server would.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import PeoplePanel from '$lib/features/shell/PeoplePanel.svelte';
import { emptyName } from '$lib/logic/person-names.ts';
import {
  BackendError,
  Workspace,
  type Invitation,
  type People,
  type WorkspaceMember,
} from '$lib/state/index.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';

afterEach(cleanup);

const MEMBERS: WorkspaceMember[] = [
  {
    userId: 'u1',
    username: 'ada',
    displayName: 'Ada',
    name: { ...emptyName(), customName: 'Ada', latinLastName: 'Lovelace', title: 'dr' },
    role: 'admin',
  },
  { userId: 'u2', username: 'grace', displayName: '', role: 'member' },
  { userId: 'u3', username: 'visitor', displayName: '', role: 'guest' },
];

function link(id: string, role: 'member' | 'guest' = 'member'): Invitation {
  return { id, role, createdBy: 'u1', createdAt: 0, expiresAt: 7 * 86_400_000, uses: 2 };
}

/** The requests the panel makes, each a mock that answers as the server would. */
function requests() {
  return {
    members: vi.fn<People['members']>(() => Promise.resolve(MEMBERS)),
    setRole: vi.fn<People['setRole']>((_workspace, userId, role) =>
      Promise.resolve({ userId, username: 'grace', displayName: '', role }),
    ),
    removeMember: vi.fn<People['removeMember']>(() => Promise.resolve()),
    invitations: vi.fn<People['invitations']>(() =>
      Promise.resolve([link('l1'), link('l2', 'guest')]),
    ),
    inviteMembers: vi.fn<People['inviteMembers']>(() =>
      Promise.resolve({ ...link('l3'), token: 'the-token' }),
    ),
    inviteGuest: vi.fn<People['inviteGuest']>(() => Promise.reject(new Error('not here'))),
    revokeInvitation: vi.fn<People['revokeInvitation']>(() => Promise.resolve()),
    guestInvitations: vi.fn<People['guestInvitations']>(() => Promise.resolve(true)),
    setGuestInvitations: vi.fn<People['setGuestInvitations']>((_workspace, enabled) =>
      Promise.resolve(enabled),
    ),
  };
}

function makeWorkspace(): Workspace {
  const { world, readerId } = seedWorld([]);
  const workspace = new Workspace({
    backend: new MemoryBackend(world, readerId),
    locale: () => 'en',
  });
  workspace.status = 'ready';
  workspace.me = {
    id: 'u1',
    username: 'ada',
    displayName: 'Ada',
    email: '',
    createdAt: 0,
    updatedAt: 0,
  };
  workspace.workspaces = [{ id: 'w1', title: 'Acme', role: 'admin' }];
  workspace.info = { id: 'w1', title: 'Acme', role: 'admin' };
  workspace.settings = { ...workspace.settings, timeZone: 'UTC', hourCycle: 'h23' };
  return workspace;
}

function mount(overrides: Partial<ReturnType<typeof requests>> = {}) {
  const people = { ...requests(), ...overrides };
  const workspace = makeWorkspace();
  render(PeoplePanel, {
    props: { workspace, people, inviteLink: (token: string) => `https://here/invite/${token}` },
  });
  return people;
}

describe('the people of a workspace', () => {
  it('lists everybody with what they are, and offers each the roles a change can make', async () => {
    mount();
    const list = await screen.findByTestId('people-members');
    expect(screen.getByRole('heading', { name: 'People in Acme' })).toBeVisible();
    expect(screen.getByText(/Anyone who opens the link can join Acme/)).toBeVisible();
    // The title is in the list; the login name beside a name shown, not for one without.
    expect(within(list).getByRole('button', { name: 'Dr. Ada @ada' })).toBeVisible();
    expect(within(list).getByRole('button', { name: 'grace' })).toBeVisible();
    const options = (name: string): string[] =>
      within(screen.getByLabelText(`Role of ${name}`))
        .getAllByRole('option')
        .map((option) => option.textContent);
    expect(options('Ada (@ada)')).toEqual(['Admin', 'Member']);
    // Their own role is another admin's to change.
    expect(screen.getByLabelText('Role of Ada (@ada)')).toBeDisabled();
    expect(options('grace')).toEqual(['Admin', 'Member', 'Guest']);
    // A guest becomes a member by a link, not here.
    expect(screen.getByLabelText('Role of visitor')).toBeDisabled();
    // Nobody takes themselves out.
    expect(screen.getByRole('button', { name: 'Remove Ada (@ada)' })).toBeDisabled();
    // The links for members, and those for the guests of boards, each to be revoked.
    expect(within(screen.getByTestId('people-links')).getAllByRole('listitem')).toHaveLength(1);
    expect(within(screen.getByTestId('people-guest-links')).getAllByRole('listitem')).toHaveLength(
      1,
    );
  });

  it("opens a member's profile from their name, and Escape gives the focus back", async () => {
    mount();
    const list = await screen.findByTestId('people-members');
    const opener = within(list).getByRole('button', { name: 'Dr. Ada @ada' });
    expect(opener).toHaveAttribute('aria-haspopup', 'dialog');
    opener.focus();
    await fireEvent.click(opener);
    const card = await screen.findByRole('dialog', { name: 'Dr. Ada' });
    expect(within(card).getByText('In Latin letters').nextElementSibling).toHaveTextContent(
      'Lovelace',
    );
    expect(within(card).getByText('Title').nextElementSibling).toHaveTextContent('Dr.');
    expect(within(card).queryByText('Degrees and qualifications')).toBeNull();
    await fireEvent.keyDown(card, { key: 'Escape' });
    await waitFor(() => {
      expect(screen.queryByRole('dialog')).toBeNull();
    });
    await waitFor(() => {
      expect(opener).toHaveFocus();
    });
  });

  it('shows the address of a link once it is made, and revokes one', async () => {
    const people = mount();
    await fireEvent.click(await screen.findByTestId('people-make-link'));
    expect(await screen.findByTestId('people-link')).toHaveValue('https://here/invite/the-token');
    expect(people.inviteMembers).toHaveBeenCalledExactlyOnceWith('w1');
    // The lists are read again after the link is made; then the row can be used.
    const revoke = within(screen.getByTestId('people-links')).getByRole('button', {
      name: 'Revoke link',
    });
    await waitFor(() => {
      expect(revoke).toBeEnabled();
    });
    await fireEvent.click(revoke);
    await waitFor(() => {
      expect(people.revokeInvitation).toHaveBeenCalledExactlyOnceWith('w1', 'l1');
    });
    // Another link than the one just made was revoked: that one is still to be copied.
    expect(screen.getByTestId('people-link')).toHaveValue('https://here/invite/the-token');
  });

  it('says why a change of role was refused, and shows what the member is again', async () => {
    const people = mount({
      setRole: vi.fn<People['setRole']>(() =>
        Promise.reject(new BackendError(409, { detail: 'no', reason: 'last_admin' })),
      ),
    });
    const role = (): HTMLElement => screen.getByLabelText('Role of grace');
    await screen.findByTestId('people-members');
    await fireEvent.change(role(), { target: { value: 'admin' } });
    expect(await screen.findByTestId('people-problem')).toHaveTextContent(
      /There must be an admin of .*. Make somebody else one first./,
    );
    expect(people.setRole).toHaveBeenCalledExactlyOnceWith('w1', 'u2', 'admin');
    expect(people.members).toHaveBeenCalledTimes(2);
    await waitFor(() => {
      expect(role()).toHaveValue('member');
    });
  });

  it('asks before a member is made a guest, and leaves them a member when it is not confirmed', async () => {
    const people = mount();
    await screen.findByTestId('people-members');
    await fireEvent.change(screen.getByLabelText('Role of grace'), { target: { value: 'guest' } });
    const ask = await screen.findByRole('alertdialog');
    expect(ask).toHaveTextContent('Make grace a guest?');
    expect(people.setRole).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Role of grace')).toHaveValue('member');
    await fireEvent.click(within(ask).getByRole('button', { name: 'Make a guest' }));
    await waitFor(() => {
      expect(people.setRole).toHaveBeenCalledExactlyOnceWith('w1', 'u2', 'guest');
    });
  });

  it('switches whether guests are invited', async () => {
    const people = mount();
    await fireEvent.click(await screen.findByTestId('people-guests'));
    await waitFor(() => {
      expect(people.setGuestInvitations).toHaveBeenCalledExactlyOnceWith('w1', false);
    });
  });

  it('says so when the people cannot be read, and tries again', async () => {
    const members = vi
      .fn<People['members']>()
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValue(MEMBERS);
    mount({ members });
    await fireEvent.click(await screen.findByRole('button', { name: 'Try again' }));
    expect(await screen.findByTestId('people-members')).toBeInTheDocument();
  });
});
