/** The teams of a workspace, against requests that answer as the server would. */
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';

import TeamsPanel from '$lib/features/shell/TeamsPanel.svelte';
import { BackendError, Workspace, type People } from '$lib/state/index.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';
import type { Team, Teams } from '$lib/state/team-types.ts';

afterEach(cleanup);

function makeWorkspace(): Workspace {
  const { world, readerId } = seedWorld([]);
  const workspace = new Workspace({
    backend: new MemoryBackend(world, readerId),
    locale: () => 'en',
  });
  workspace.status = 'ready';
  workspace.workspaces = [{ id: 'w1', role: 'admin', title: 'Studio' }];
  workspace.info = { id: 'w1', role: 'admin', title: 'Studio' };
  return workspace;
}

function makePeople(): People {
  return {
    members: () => Promise.resolve([]),
    setRole: () => Promise.reject(new Error('unused')),
    removeMember: () => Promise.resolve(),
    invitations: () => Promise.resolve([]),
    inviteMembers: () => Promise.reject(new Error('unused')),
    inviteGuest: () => Promise.reject(new Error('unused')),
    revokeInvitation: () => Promise.resolve(),
    guestInvitations: () => Promise.resolve(true),
    setGuestInvitations: () => Promise.resolve(true),
  };
}

function makeTeams(overrides: Partial<Teams>): Teams {
  return {
    list: () => Promise.resolve([]),
    create: () => Promise.reject(new Error('unused')),
    rename: () => Promise.reject(new Error('unused')),
    delete: () => Promise.resolve(),
    addMember: () => Promise.resolve(),
    removeMember: () => Promise.resolve(),
    onContainer: () => Promise.resolve([]),
    assign: () => Promise.resolve(),
    unassign: () => Promise.resolve(),
    ...overrides,
  };
}

const REFUSALS: [problem: BackendError, told: RegExp][] = [
  [
    new BackendError(400, {
      errors: [{ pointer: '/name', reason: 'too_long', detail: 'too long' }],
    }),
    /up to 64 characters/,
  ],
  [
    new BackendError(400, {
      errors: [{ pointer: '/name', reason: 'invalid_characters', detail: 'invisible' }],
    }),
    /cannot have invisible characters/,
  ],
  [new BackendError(409, { reason: 'team_name_taken' }), /Letter case does not make/],
];

it.each(REFUSALS)(
  'keeps a refused name typed and says why in one line (%#)',
  async (problem, told) => {
    const teams = makeTeams({
      list: vi.fn<Teams['list']>(() => Promise.resolve([])),
      create: vi.fn<Teams['create']>(() => Promise.reject(problem)),
    });
    const people = makePeople();
    const workspace = makeWorkspace();
    render(TeamsPanel, { workspace, people, teams });
    const input = await screen.findByRole('textbox', { name: 'Group name' });
    await fireEvent.input(input, { target: { value: 'DESIGN' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Create group' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(told);
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(input).toHaveValue('DESIGN');
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAccessibleDescription(told);
  },
);

it('says why a new name of a team is refused, and keeps the rename box with what was typed', async () => {
  const design: Team = { id: 't1', workspaceId: 'w1', name: 'Design', members: [] };
  const teams = makeTeams({
    list: vi.fn<Teams['list']>(() => Promise.resolve([design])),
    rename: vi.fn<Teams['rename']>(() =>
      Promise.reject(new BackendError(409, { reason: 'team_name_taken' })),
    ),
  });
  const people = makePeople();
  const workspace = makeWorkspace();
  render(TeamsPanel, { workspace, people, teams });
  await fireEvent.click(await screen.findByText('Design'));
  await fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
  const box = screen.getByRole('textbox', { name: 'New name for Design' });
  await fireEvent.input(box, { target: { value: 'RESEARCH' } });
  await fireEvent.click(screen.getByRole('button', { name: 'Save name' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(/Letter case does not make/);
  expect(box).toHaveValue('RESEARCH');
  expect(box).toHaveAttribute('aria-invalid', 'true');
  expect(box).toHaveAccessibleDescription(/already exists here/);
  expect(screen.getByRole('textbox', { name: 'Group name' })).not.toHaveAttribute('aria-invalid');
  await fireEvent.input(box, { target: { value: 'Research' } });
  expect(box).not.toHaveAttribute('aria-invalid');
  expect(box).not.toHaveAccessibleDescription(/already exists here/);
});

it('opens the folded groups to say they could not be read', async () => {
  const teams = makeTeams({
    list: vi.fn<Teams['list']>(() => Promise.reject(new BackendError(500, { detail: 'down' }))),
  });
  render(TeamsPanel, { workspace: makeWorkspace(), people: makePeople(), teams, foldable: true });
  expect(await screen.findByRole('alert')).toHaveTextContent(/could not/i);
  expect(screen.getByTestId('teams-toggle')).toHaveAttribute('aria-expanded', 'true');
});
