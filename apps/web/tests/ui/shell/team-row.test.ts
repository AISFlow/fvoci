import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import TeamRow from '$lib/features/shell/TeamRow.svelte';

afterEach(cleanup);

it('disables a temporary administrator adding themselves but allows another person', async () => {
  const onAdd = vi.fn(() => Promise.resolve(true));
  render(TeamRow, {
    team: { id: 'team', workspaceId: 'workspace', name: 'Design', members: [] },
    members: [
      { userId: 'me', username: 'ada', displayName: 'Ada', role: 'admin' },
      { userId: 'other', username: 'grace', displayName: 'Grace', role: 'member' },
    ],
    managing: true,
    working: false,
    restrictedSelf: 'me',
    onAdd,
    onRename: () => Promise.resolve(true),
    onRemove: () => Promise.resolve(true),
    onDelete: vi.fn(),
  });
  await fireEvent.click(screen.getByText('Design'));
  expect(screen.getByRole('option', { name: /Ada/ })).toBeDisabled();
  expect(screen.getByRole('option', { name: /Grace/ })).toBeEnabled();
  expect(screen.getByText(/You cannot add yourself to a group/)).toBeInTheDocument();
  await fireEvent.change(screen.getByRole('combobox'), { target: { value: 'other' } });
  await fireEvent.click(screen.getByRole('button', { name: 'Add a person' }));
  expect(onAdd).toHaveBeenCalledWith('other');
});

it('keeps the rename box and the typed name open when the new name is refused', async () => {
  const onRename = vi.fn(() => Promise.resolve(false));
  render(TeamRow, {
    team: { id: 'team', workspaceId: 'workspace', name: 'Design', members: [] },
    members: [],
    managing: true,
    working: false,
    onAdd: () => Promise.resolve(true),
    onRename,
    onRemove: () => Promise.resolve(true),
    onDelete: vi.fn(),
  });
  await fireEvent.click(screen.getByText('Design'));
  await fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
  const input = screen.getByRole('textbox', { name: 'New name for Design' });
  await fireEvent.input(input, { target: { value: 'Taken' } });
  await fireEvent.click(screen.getByRole('button', { name: 'Save name' }));
  expect(onRename).toHaveBeenCalledWith('Taken');
  await Promise.resolve();
  expect(screen.getByRole('textbox', { name: 'New name for Design' })).toHaveValue('Taken');
});
