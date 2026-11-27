/**
 * Deleting the account: what the settings say before anything is asked, what
 * the request sends, what a refusal is told as, and what the account that
 * waits is shown.
 */
import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import AccountDeletion from '$lib/features/account/AccountDeletion.svelte';
import AccountLeaving from '$lib/features/account/AccountLeaving.svelte';
import {
  daysLeft,
  deletionProblemOf,
  waitsNoMore,
} from '$lib/features/account/account-deletion.ts';
import { refusedMember } from '$lib/state/backend.ts';
import { BackendError, type AccountDeletion as Standing } from '$lib/state/index.ts';

afterEach(cleanup);

const DAY = 24 * 60 * 60 * 1000;

const open: Standing = {
  graceDays: 30,
  proof: 'password',
  lastInstanceAdmin: false,
  soleAdminOf: [],
  organizations: [],
};

async function opened(standing: Standing, onDelete = vi.fn(() => Promise.resolve())) {
  render(AccountDeletion, {
    props: { username: 'hana', read: () => Promise.resolve(standing), onDelete },
  });
  await fireEvent.click(screen.getByRole('button', { name: 'Delete account' }));
  return onDelete;
}

describe('the settings', () => {
  it('say what the deletion comes to, and ask with the name and the password', async () => {
    const onDelete = await opened(open);
    expect(await screen.findByTestId('account-deletion-text')).toHaveTextContent(
      'Your account is deleted 30 days after you ask.',
    );
    expect(screen.queryByTestId('account-deletion-organizations')).toBeNull();

    // Another name is told at its field, and nothing is asked.
    const name = screen.getByLabelText('Type your user name, hana, to confirm');
    await fireEvent.input(name, { target: { value: 'hans' } });
    await fireEvent.input(screen.getByLabelText('Current password'), {
      target: { value: 'the password' },
    });
    const submit = screen.getByRole('button', { name: 'Delete my account' });
    await fireEvent.click(submit);
    expect(screen.getByRole('alert')).toHaveTextContent("That is not this account's user name.");
    expect(name).toHaveAttribute('aria-invalid', 'true');
    expect(onDelete).not.toHaveBeenCalled();

    // In any letter case, as the server compares.
    await fireEvent.input(name, { target: { value: ' Hana ' } });
    await fireEvent.click(submit);
    expect(onDelete).toHaveBeenCalledExactlyOnceWith('Hana', 'the password');
    expect(submit).toBeDisabled();
  });

  it('name the organizations that go with the account, and how much is in each', async () => {
    await opened({
      ...open,
      organizations: [
        { id: 'a', kind: 'shared', name: 'Alone', boards: 3, wikis: 1, files: 12 },
        { id: 'd', kind: 'shared', name: 'Small', boards: 1, wikis: 0, files: 1 },
        { id: 'b', kind: 'shared', name: 'Walled' },
        { id: 'c', kind: 'shared', name: '', boards: 0, wikis: 0, files: 0 },
      ],
    });
    const list = await screen.findByTestId('account-deletion-organizations');
    expect(list).toHaveTextContent('They are deleted when your account is');
    expect(list).toHaveTextContent('Alone: 3 boards, 1 wiki, 12 files');
    expect(list).toHaveTextContent('Small: 1 board, 0 wikis, 1 file');
    expect(list).toHaveTextContent('Walled: sign in through its single sign-on');
    expect(list).toHaveTextContent('Unnamed organization: 0 boards, 0 wikis, 0 files');
  });

  it('say that the own space goes with the account, apart from the organizations with nobody else', async () => {
    await opened({
      ...open,
      organizations: [
        { id: 'a', kind: 'shared', name: 'Alone', boards: 1, wikis: 0, files: 0 },
        { id: 'own', kind: 'personal', name: '', boards: 4, wikis: 1, diaries: 1, files: 2 },
      ],
    });
    expect(await screen.findByTestId('account-deletion-own-space')).toHaveTextContent(
      'My space is deleted with your account, with everything in it: 4 boards, 1 wiki, 1 diary, 2 files',
    );
    const list = screen.getByTestId('account-deletion-organizations');
    expect(within(list).getAllByRole('listitem')).toHaveLength(1);
    expect(list).toHaveTextContent('Alone: 1 board, 0 wikis, 0 files');
    expect(list).not.toHaveTextContent('Unnamed');
  });

  it('say the own space alone where nothing else goes with the account', async () => {
    await opened({
      ...open,
      organizations: [{ id: 'own', kind: 'personal', name: '' }],
    });
    expect(await screen.findByTestId('account-deletion-own-space')).toHaveTextContent(
      'My space is deleted with your account, with everything in it.',
    );
    expect(screen.queryByTestId('account-deletion-organizations')).toBeNull();
  });

  it('give the only administrator no form, and say of what', async () => {
    await opened({
      ...open,
      lastInstanceAdmin: true,
      soleAdminOf: [{ id: 'a', name: 'Acme' }],
    });
    const held = await screen.findByTestId('account-deletion-held');
    expect(held).toHaveTextContent('You are the only administrator of this server.');
    expect(held).toHaveTextContent('Acme');
    expect(screen.queryByRole('button', { name: 'Delete my account' })).toBeNull();
  });

  it('ask an account without a password for no password', async () => {
    const onDelete = await opened({ ...open, proof: 'sign_in' });
    await fireEvent.input(await screen.findByLabelText('Type your user name, hana, to confirm'), {
      target: { value: 'hana' },
    });
    expect(screen.queryByLabelText('Current password')).toBeNull();
    expect(screen.getByText(/This account has no password/u)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Delete my account' }));
    expect(onDelete).toHaveBeenCalledExactlyOnceWith('hana', '');
  });

  it('tell a refusal at its field and leave the form to be sent again', async () => {
    const wrong = new BackendError(400, {
      detail: 'no',
      reason: 'invalid_credentials',
      errors: [refusedMember('password', 'wrong', 'invalid_credentials')],
    });
    const onDelete = await opened(
      open,
      vi.fn(() => Promise.reject(wrong)),
    );
    await fireEvent.input(await screen.findByLabelText('Type your user name, hana, to confirm'), {
      target: { value: 'hana' },
    });
    const password = screen.getByLabelText('Current password');
    await fireEvent.input(password, { target: { value: 'not it' } });
    const submit = screen.getByRole('button', { name: 'Delete my account' });
    await fireEvent.click(submit);
    expect(await screen.findByRole('alert')).toHaveTextContent('The current password is wrong.');
    expect(password).toHaveAttribute('aria-invalid', 'true');
    expect(submit).toBeEnabled();
    expect(onDelete).toHaveBeenCalledTimes(1);
  });

  it('give the focus back to the button after a refusal that is about no field', async () => {
    const onDelete = await opened(
      open,
      vi.fn(() => Promise.reject(new BackendError(429, { detail: 'no' }))),
    );
    await fireEvent.input(await screen.findByLabelText('Type your user name, hana, to confirm'), {
      target: { value: 'hana' },
    });
    await fireEvent.input(screen.getByLabelText('Current password'), { target: { value: 'it' } });
    const submit = screen.getByRole('button', { name: 'Delete my account' });
    await fireEvent.click(submit);
    expect(await screen.findByRole('alert')).toHaveTextContent('Too many tries.');
    await vi.waitFor(() => {
      expect(submit).toHaveFocus();
    });
    expect(onDelete).toHaveBeenCalledTimes(1);
  });

  it('read again when the account became the only administrator of something, and say of what', async () => {
    const held: Standing = { ...open, soleAdminOf: [{ id: 'a', name: 'Acme' }] };
    const read = vi
      .fn<() => Promise<Standing>>()
      .mockResolvedValueOnce(open)
      .mockResolvedValue(held);
    const onDelete = vi.fn(() =>
      Promise.reject(new BackendError(409, { detail: 'no', reason: 'last_org_admin', orgId: 'a' })),
    );
    render(AccountDeletion, { props: { username: 'hana', read, onDelete } });
    await fireEvent.click(screen.getByRole('button', { name: 'Delete account' }));
    await fireEvent.input(await screen.findByLabelText('Type your user name, hana, to confirm'), {
      target: { value: 'hana' },
    });
    await fireEvent.input(screen.getByLabelText('Current password'), { target: { value: 'it' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Delete my account' }));
    expect(await screen.findByTestId('account-deletion-held')).toHaveTextContent('Acme');
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('cannot be folded while the request is on its way, and let go of a reading that ends folded', async () => {
    let answer: (standing: Standing) => void = () => undefined;
    const read = vi.fn(() => new Promise<Standing>((resolve) => (answer = resolve)));
    const onDelete = vi.fn(() => new Promise<void>(() => undefined));
    render(AccountDeletion, { props: { username: 'hana', read, onDelete } });
    const toggle = screen.getByRole('button', { name: 'Delete account' });
    // Folded before the reading ends: nothing of it shows when it is opened again.
    await fireEvent.click(toggle);
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    answer({ ...open, lastInstanceAdmin: true });
    await Promise.resolve();
    expect(screen.queryByTestId('account-deletion-held')).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Delete account' }));
    answer(open);
    await fireEvent.input(await screen.findByLabelText('Type your user name, hana, to confirm'), {
      target: { value: 'hana' },
    });
    await fireEvent.input(screen.getByLabelText('Current password'), { target: { value: 'it' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Delete my account' }));
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
  });
});

describe('a refused request', () => {
  const refusal = (status: 400 | 401 | 403 | 409 | 429 | 500, extra: object = {}) =>
    new BackendError(status, { detail: 'no', ...extra });

  it('is told by its reason', () => {
    expect(
      deletionProblemOf(
        refusal(400, { errors: [refusedMember('username', 'another', 'invalid')] }),
      ),
    ).toEqual({ text: "That is not this account's user name.", field: 'username' });
    expect(deletionProblemOf(refusal(403, { reason: 'sign_in_again' })).text).toBe(
      'Sign in through single sign-on again, then ask within five minutes.',
    );
    expect(deletionProblemOf(refusal(401, { reason: 'sso_required', orgId: 'a' })).text).toMatch(
      /opens only for its single sign-on/u,
    );
    expect(deletionProblemOf(refusal(409, { reason: 'last_admin' })).text).toMatch(
      /only administrator of this server/u,
    );
    expect(deletionProblemOf(refusal(409, { reason: 'last_org_admin', orgId: 'a' })).text).toMatch(
      /only administrator of an organization/u,
    );
    expect(deletionProblemOf(refusal(429)).text).toBe(
      'Too many tries. Wait a moment and try again.',
    );
    expect(waitsNoMore(refusal(409))).toBe(true);
    expect(waitsNoMore(refusal(401))).toBe(true);
    expect(waitsNoMore(refusal(401, { reason: 'account_deletion_pending' }))).toBe(false);
    expect(waitsNoMore(refusal(500))).toBe(false);
    expect(waitsNoMore(new Error('offline'))).toBe(false);
    for (const other of [refusal(409), refusal(500), new Error('offline')]) {
      expect(deletionProblemOf(other)).toEqual({
        text: 'That did not work. Try again.',
        field: undefined,
      });
    }
  });
});

describe('the days an account still waits', () => {
  it('count a day that has begun whole, and none once it is over', () => {
    const now = 1_800_000_000_000;
    expect(daysLeft(now + 30 * DAY, now)).toBe(30);
    expect(daysLeft(now + 29 * DAY + 1, now)).toBe(30);
    expect(daysLeft(now + 1, now)).toBe(1);
    expect(daysLeft(now, now)).toBe(0);
    expect(daysLeft(now - DAY, now)).toBe(0);
  });
});

describe('the account that waits', () => {
  const waiting: Standing = {
    ...open,
    requestedAt: Date.now() - DAY,
    deleteAfter: Date.now() + 29 * DAY - 1000,
    organizations: [
      { id: 'own', kind: 'personal', name: '', boards: 2, wikis: 0, files: 0 },
      { id: 'a', kind: 'shared', name: 'Alone', boards: 1, wikis: 0, files: 0 },
    ],
  };

  it('is told when it is deleted and what goes with it, and takes the deletion back', async () => {
    const onCancel = vi.fn(() => Promise.resolve());
    const onSignOut = vi.fn();
    render(AccountLeaving, {
      props: {
        read: () => Promise.resolve(waiting),
        onCancel,
        onStale: vi.fn(),
        onSignOut,
        locale: 'en',
      },
    });
    expect(screen.getByRole('heading')).toHaveTextContent('This account is going to be deleted');
    expect(await screen.findByTestId('account-leaving-text')).toHaveTextContent(
      /It is deleted in 29 days, on \w+ \d+, \d{4}\./u,
    );
    expect(screen.getByTestId('account-leaving-own-space')).toHaveTextContent(
      'My space is deleted with it, with everything in it.',
    );
    const alone = screen.getByTestId('account-leaving-organizations');
    expect(alone).toHaveTextContent('Alone');
    expect(alone).not.toHaveTextContent('Unnamed');

    await fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
    expect(onSignOut).toHaveBeenCalledTimes(1);
    const cancel = screen.getByRole('button', { name: 'Cancel the deletion' });
    await fireEvent.click(cancel);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(cancel).toBeDisabled();
  });

  it('can take the deletion back again when that failed, and after its days are over', async () => {
    const onCancel = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new BackendError(500, { detail: 'no' }))
      .mockResolvedValueOnce();
    render(AccountLeaving, {
      props: {
        read: () => Promise.resolve({ ...waiting, deleteAfter: Date.now() - 1000 }),
        onCancel,
        onStale: vi.fn(),
        onSignOut: vi.fn(),
        locale: 'en',
      },
    });
    expect(await screen.findByTestId('account-leaving-text')).toHaveTextContent(
      'Its waiting time is over',
    );
    const cancel = screen.getByRole('button', { name: 'Cancel the deletion' });
    await fireEvent.click(cancel);
    expect(
      await screen.findByText('The deletion could not be cancelled. Try again.'),
    ).toBeVisible();
    expect(cancel).toBeEnabled();
    await vi.waitFor(() => {
      expect(cancel).toHaveFocus();
    });
    await fireEvent.click(cancel);
    expect(onCancel).toHaveBeenCalledTimes(2);
  });

  it('says what holds the deletion back, when the account became the only administrator meanwhile', async () => {
    render(AccountLeaving, {
      props: {
        read: () =>
          Promise.resolve({
            ...waiting,
            lastInstanceAdmin: true,
            soleAdminOf: [{ id: 'a', name: 'Acme' }],
            organizations: [],
          }),
        onCancel: vi.fn(() => Promise.resolve()),
        onStale: vi.fn(),
        onSignOut: vi.fn(),
        locale: 'en',
      },
    });
    const held = await screen.findByTestId('account-leaving-held');
    expect(held).toHaveTextContent('It is not deleted while you are the only administrator');
    expect(held).toHaveTextContent('This server');
    expect(held).toHaveTextContent('Acme');
    expect(screen.queryByTestId('account-leaving-organizations')).toBeNull();
  });

  it('says one day as one day', async () => {
    render(AccountLeaving, {
      props: {
        read: () => Promise.resolve({ ...waiting, deleteAfter: Date.now() + DAY - 1000 }),
        onCancel: vi.fn(() => Promise.resolve()),
        onStale: vi.fn(),
        onSignOut: vi.fn(),
        locale: 'en',
      },
    });
    expect(await screen.findByTestId('account-leaving-text')).toHaveTextContent(
      /It is deleted in 1 day, on /u,
    );
  });

  it('starts the page again when the account waits no more, and does not ask to try again', async () => {
    // Taken back in another tab while this page was open: the cancel finds nothing waiting.
    const onStale = vi.fn();
    const onCancel = vi.fn(() => Promise.reject(new BackendError(409, { detail: 'no' })));
    const props = { onCancel, onStale, onSignOut: vi.fn(), locale: 'en' };
    const shown = render(AccountLeaving, {
      props: { ...props, read: () => Promise.resolve(waiting) },
    });
    await fireEvent.click(await screen.findByRole('button', { name: 'Cancel the deletion' }));
    await vi.waitFor(() => {
      expect(onStale).toHaveBeenCalledTimes(1);
    });
    expect(screen.queryByText('The deletion could not be cancelled. Try again.')).toBeNull();
    shown.unmount();

    // Erased meanwhile: the session is over, and the reading says so.
    render(AccountLeaving, {
      props: { ...props, read: () => Promise.reject(new BackendError(401, { detail: 'no' })) },
    });
    await vi.waitFor(() => {
      expect(onStale).toHaveBeenCalledTimes(2);
    });
    cleanup();

    // Read as an account that does not wait (no day to be deleted on).
    render(AccountLeaving, { props: { ...props, read: () => Promise.resolve(open) } });
    await vi.waitFor(() => {
      expect(onStale).toHaveBeenCalledTimes(3);
    });
  });
});
