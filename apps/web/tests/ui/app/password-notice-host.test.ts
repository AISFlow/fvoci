/**
 * When the notice to set a password stands at the foot of the pane: in the
 * reader's own space, for an account without a password, once something is
 * made there, until it is closed for good, which the account keeps (the
 * client state) so that it does not come back. Against the backend in memory,
 * whose reader is made an account without a password.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { closeOwnSpace } from '$lib/state/memory/spaces.ts';
import { firstKanban } from '$lib/state/workspace-entry.ts';
import { BackendError, type Account } from '$lib/state/index.ts';

import PasswordNoticeHost from '../../../src/app/PasswordNoticeHost.svelte';
import { setup, settle } from '../../state/support.ts';

afterEach(cleanup);

async function shown({ own = true, hasPassword = false, made = true, kept = {} } = {}) {
  const setUp = setup();
  const { world, backend, workspace } = setUp;
  const reader = world.users.get(backend.userId);
  if (!reader) throw new Error('no reader');
  world.users.set(reader.id, { ...reader, hasPassword });
  world.screenState.set(reader.id, kept);
  const listed = await backend.workspaces();
  const at = listed.find((one) => (one.kind === 'personal') === own);
  if (!at) throw new Error('no such workspace');
  if (made) await backend.createBoard(at.id, { title: 'Mine' }, firstKanban([], 'Board'));
  await workspace.load(at.id);
  // As the server does to the provider's session that sets it: the space closes to it.
  const setPassword = vi.fn<Account['setPassword']>((password) => {
    closeOwnSpace(world, reader.id, password);
    return Promise.resolve();
  });
  const loginOptions = vi.fn(() => Promise.resolve({ registrationOpen: false, mailEnabled: true }));
  const account = { setPassword, loginOptions } as Partial<Account> as Account;
  const onProvider = vi.fn(() => Promise.resolve());
  render(PasswordNoticeHost, { props: { workspace, account, onProvider } });
  await settle();
  return { ...setUp, setPassword, readerId: reader.id };
}

describe('the notice to set a password, in the app', () => {
  it('stands in the own space of an account without a password once something is in it', async () => {
    await shown();
    expect(await screen.findByTestId('password-notice')).toBeInTheDocument();
  });

  it('is not there before anything is made, in a shared workspace, or with a password', async () => {
    for (const made of [
      { made: false },
      { own: false },
      { hasPassword: true },
      { kept: { passwordNoticeDismissed: '1' } },
    ]) {
      await shown(made);
      await settle();
      expect(screen.queryByTestId('password-notice')).toBeNull();
      cleanup();
    }
  });

  it('comes when the first thing is made, and is closed for good with the account', async () => {
    const { backend, workspace, world, readerId } = await shown({ made: false });
    expect(screen.queryByTestId('password-notice')).toBeNull();
    await workspace.createBoard({ title: 'First' }, 'Board', 'Made');
    expect(await screen.findByTestId('password-notice')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('password-notice-dismiss'));
    expect(screen.queryByTestId('password-notice')).toBeNull();
    await waitFor(async () => {
      expect((await backend.screenState()).passwordNoticeDismissed).toBe('1');
    });
    expect(world.screenState.get(readerId)).toMatchObject({ passwordNoticeDismissed: '1' });
  });

  it('sets the password through the account, and shows it in the session: the own space stays open', async () => {
    const { setPassword, workspace, backend } = await shown();
    const confirmed = vi.spyOn(backend, 'confirmPassword');
    await fireEvent.input(await screen.findByLabelText('New password'), {
      target: { value: 'a long password' },
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Set password' }));
    await waitFor(() => {
      expect(setPassword).toHaveBeenCalledExactlyOnceWith('a long password');
    });
    // Set, the account has one: the notice says so, with the name to sign in by, through
    // the entering again of the space, and the reader is not asked for the password again.
    expect(await screen.findByTestId('password-notice-done')).toHaveTextContent(
      `Your password is set. Your user name is ${workspace.me?.username ?? '?'}.`,
    );
    expect(workspace.me?.hasPassword).toBe(true);
    expect(confirmed).toHaveBeenCalledExactlyOnceWith('a long password');
    await waitFor(() => {
      expect(workspace.entered).toBe('ready');
    });
    expect(workspace.info?.kind).toBe('personal');
    expect(workspace.workspaces.some((one) => one.passwordRequired)).toBe(false);
    expect(screen.getByTestId('password-notice-done')).toBeInTheDocument();
    // Closed, nothing takes its place in this page: the line about the address is for another visit.
    await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    await settle();
    expect(screen.queryByTestId('password-notice')).toBeNull();
    expect(screen.queryByTestId('email-notice')).toBeNull();
  });

  it('has the own space ask for it where the session could not show it, wherever the reader goes and comes back', async () => {
    const { workspace, world, backend } = await shown();
    vi.spyOn(backend, 'confirmPassword').mockRejectedValue(new BackendError(429, {}));
    await fireEvent.input(await screen.findByLabelText('New password'), {
      target: { value: 'a long password' },
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Set password' }));
    await waitFor(() => {
      expect(workspace.entered).toBe('password');
    });
    const own = workspace.closed?.id ?? '';
    await workspace.enter(world.workspace.id);
    expect(workspace.entered).toBe('ready');
    await workspace.enter(own);
    await settle();
    expect(workspace.entered).toBe('password');
    // The password is set all the same, and the notice says so until it is closed.
    expect(screen.getByTestId('password-notice-done')).toHaveTextContent('Your password is set.');
    await fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(screen.queryByTestId('password-notice')).toBeNull();
  });
});
