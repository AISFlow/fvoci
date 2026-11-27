/** The teams of a board in its share dialog, against requests that answer as the server would. */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';

import ContainerTeams from '$lib/features/shell/ContainerTeams.svelte';
import { BackendError, BoardStore, Workspace } from '$lib/state/index.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';
import type { Team, Teams } from '$lib/state/team-types.ts';

afterEach(cleanup);

const DESIGN: Team = { id: 't1', workspaceId: 'w1', name: 'Design', members: [] };

function makeHarness() {
  const { world, readerId } = seedWorld([]);
  const workspace = new Workspace({
    backend: new MemoryBackend(world, readerId),
    locale: () => 'en',
  });
  workspace.status = 'ready';
  workspace.workspaces = [{ id: 'w1', title: 'Acme', role: 'admin' }];
  workspace.info = { id: 'w1', title: 'Acme', role: 'admin' };
  const board = new BoardStore(workspace, 'b1');
  return { workspace, board };
}

function makeTeams(overrides: Partial<Teams>): Teams {
  return {
    list: () => Promise.resolve([DESIGN]),
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

it('puts a refused role back in its select, with the reason', async () => {
  const assign = vi.fn<Teams['assign']>(() => Promise.reject(new BackendError(403)));
  const teams = makeTeams({
    list: vi.fn<Teams['list']>(() => Promise.resolve([DESIGN])),
    onContainer: vi.fn<Teams['onContainer']>(() =>
      Promise.resolve([{ team: DESIGN, role: 'viewer' }]),
    ),
    assign,
  });
  const { workspace, board } = makeHarness();
  render(ContainerTeams, { workspace, shareable: board, teams });
  const select = await screen.findByRole<HTMLSelectElement>('combobox', { name: /Design/ });
  expect(select.value).toBe('viewer');
  await fireEvent.change(select, { target: { value: 'editor' } });
  expect(assign).toHaveBeenCalledWith('w1', 'b1', 't1', 'editor');
  expect(await screen.findByRole('alert')).toHaveTextContent('Could not change the group.');
  await waitFor(() => {
    expect(select.value).toBe('viewer');
  });
});

it('takes a team off the board by the × named after it', async () => {
  const unassign = vi.fn<Teams['unassign']>(() => Promise.resolve());
  const teams = makeTeams({
    list: vi.fn<Teams['list']>(() => Promise.resolve([DESIGN])),
    onContainer: vi.fn<Teams['onContainer']>(() =>
      Promise.resolve([{ team: DESIGN, role: 'editor' }]),
    ),
    unassign,
  });
  const { workspace, board } = makeHarness();
  render(ContainerTeams, { workspace, shareable: board, teams });
  const remove = await screen.findByRole('button', { name: 'Remove Design from board' });
  expect(remove).not.toHaveTextContent('Design');
  await fireEvent.click(remove);
  expect(unassign).toHaveBeenCalledWith('w1', 'b1', 't1');
});

it('shows the teams and their roles, unchanged, to a reader who changes nothing', async () => {
  const teams = makeTeams({
    list: vi.fn<Teams['list']>(() => Promise.resolve([DESIGN])),
    onContainer: vi.fn<Teams['onContainer']>(() =>
      Promise.resolve([{ team: DESIGN, role: 'viewer' }]),
    ),
  });
  const { workspace, board } = makeHarness();
  render(ContainerTeams, { workspace, shareable: board, teams, readOnly: true });
  expect(await screen.findByText('Design')).toBeInTheDocument();
  expect(screen.getByText('Viewer')).toBeInTheDocument();
  expect(screen.queryByRole('combobox')).toBeNull();
  expect(screen.queryByRole('button', { name: /Remove/ })).toBeNull();
});

it('leaves out an empty list of teams for a reader who changes nothing', async () => {
  const onContainer = vi.fn<Teams['onContainer']>(() => Promise.resolve([]));
  const teams = makeTeams({
    list: vi.fn<Teams['list']>(() => Promise.resolve([DESIGN])),
    onContainer,
  });
  const { workspace, board } = makeHarness();
  render(ContainerTeams, { workspace, shareable: board, teams, readOnly: true });
  await waitFor(() => {
    expect(onContainer).toHaveBeenCalled();
  });
  expect(screen.queryByTestId('board-teams')).toBeNull();
});

it('stays open when the last team is taken off the board, with the way to add one', async () => {
  let onBoard = [{ team: DESIGN, role: 'editor' as const }];
  const teams = makeTeams({
    onContainer: vi.fn<Teams['onContainer']>(() => Promise.resolve(onBoard)),
    unassign: vi.fn<Teams['unassign']>(() => {
      onBoard = [];
      return Promise.resolve();
    }),
  });
  const { workspace, board } = makeHarness();
  render(ContainerTeams, { workspace, shareable: board, teams, foldable: true });
  const remove = await screen.findByRole('button', { name: 'Remove Design from board' });
  expect(screen.getByTestId('board-teams-toggle')).toHaveAttribute('aria-expanded', 'true');
  await fireEvent.click(remove);
  await waitFor(() => {
    expect(screen.queryByRole('button', { name: 'Remove Design from board' })).toBeNull();
  });
  expect(screen.getByTestId('board-teams-toggle')).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getAllByRole('combobox').length).toBeGreaterThan(0);
});
