/**
 * The account's address in the settings: asked to change with the password,
 * what a change that waits offers, what a refusal says and where; the page
 * of the link that changes it, which waits for its button; and the line of
 * the reader's own space.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import type { ComponentProps } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import ConfirmEmailPage from '$lib/features/account/ConfirmEmailPage.svelte';
import EmailChange from '$lib/features/account/EmailChange.svelte';
import { BackendError, type Account, type EmailChange as Changes } from '$lib/state/index.ts';
import { firstKanban } from '$lib/state/workspace-entry.ts';

import EmailChangeHost from '../../../src/app/EmailChangeHost.svelte';
import PasswordNoticeHost from '../../../src/app/PasswordNoticeHost.svelte';
import { parseRoute } from '../../../src/app/router.svelte.ts';
import { setup, settle } from '../../state/support.ts';

afterEach(cleanup);

const OLD = 'ada@company.example';
const NEW = 'ada@home.example';

function section(props: Partial<ComponentProps<typeof EmailChange>> = {}) {
  const onRequest = vi.fn<(email: string, password: string) => Promise<void>>(() =>
    Promise.resolve(),
  );
  const onResend = vi.fn(() => Promise.resolve());
  const onCancel = vi.fn(() => Promise.resolve());
  const given = {
    email: OLD,
    pending: undefined,
    hasPassword: true,
    mails: true,
    onRequest,
    onResend,
    onCancel,
    ...props,
  };
  return { ...render(EmailChange, { props: given }), ...given };
}

async function ask(email: string, password: string): Promise<void> {
  await fireEvent.click(screen.getByRole('button', { name: 'Change address' }));
  await fireEvent.input(screen.getByLabelText('New email address'), { target: { value: email } });
  await fireEvent.input(screen.getByLabelText('Current password'), {
    target: { value: password },
  });
  await fireEvent.click(screen.getByRole('button', { name: 'Change address' }));
}

describe("the account's address in the settings", () => {
  it('shows the address, and asks for a new one with the password', async () => {
    const { onRequest, rerender } = section();
    expect(screen.getByTestId('email-change-address')).toHaveTextContent(OLD);
    expect(screen.queryByLabelText('New email address')).toBeNull();
    await ask(NEW, 'the password');
    expect(onRequest).toHaveBeenCalledExactlyOnceWith(NEW, 'the password');
    // Asked, the form folds; the address is the old one until the link is opened.
    await waitFor(() => {
      expect(screen.queryByLabelText('New email address')).toBeNull();
    });
    expect(screen.getByRole('status')).toHaveTextContent(/^$/);
    await rerender({ pending: NEW });
    expect(screen.getByTestId('email-change-pending')).toHaveTextContent(NEW);
    expect(screen.getByTestId('email-change-address')).toHaveTextContent(OLD);
  });

  it('says the address is changed where the server changed it with the password alone', async () => {
    // As the host does: the address is read anew before the asking resolves.
    const shown: { rerender?: (props: { email: string }) => Promise<void> } = {};
    const onRequest = vi.fn(async () => {
      await shown.rerender?.({ email: 'Ada@Home.example' });
    });
    shown.rerender = section({ onRequest }).rerender;
    await ask(` ${NEW} `, 'the password');
    expect(await screen.findByText('Your address is changed.')).toBeInTheDocument();
    expect(screen.queryByTestId('email-change-pending')).toBeNull();
  });

  it('says that it was asked, to a screen reader, once the change waits', async () => {
    // As the host does: what waits is read anew before the asking resolves.
    const shown: { rerender?: (props: { pending: string }) => Promise<void> } = {};
    const onRequest = vi.fn(async () => {
      await shown.rerender?.({ pending: NEW });
    });
    shown.rerender = section({ onRequest }).rerender;
    await ask(NEW, 'the password');
    await waitFor(() => {
      expect(screen.getByRole('status')).toHaveTextContent(
        'Asked. The address changes when the link mailed to the new address is opened.',
      );
    });
    // The box says it to the eye.
    expect(screen.getByRole('status')).toHaveClass('sr-only');
    expect(screen.getByTestId('email-change-pending')).toHaveTextContent(NEW);
  });

  it('mails the link again and takes the change back, and says which', async () => {
    const { onResend, onCancel, rerender } = section({ pending: NEW });
    await fireEvent.click(screen.getByRole('button', { name: 'Send the link again' }));
    expect(onResend).toHaveBeenCalledOnce();
    expect(
      await screen.findByText(/Asked for again. A new link ends the one before it./),
    ).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel the change' }));
    expect(onCancel).toHaveBeenCalledOnce();
    await rerender({ pending: undefined });
    expect(await screen.findByText(/The change is cancelled/)).toBeInTheDocument();
    expect(screen.queryByTestId('email-change-pending')).toBeNull();
  });

  it('says that nothing waits any more where the change was made or ended elsewhere', async () => {
    const onResend = vi.fn(() => Promise.reject(new BackendError(404, {})));
    section({ pending: NEW, onResend });
    await fireEvent.click(screen.getByRole('button', { name: 'Send the link again' }));
    expect(await screen.findByText('No change is waiting any more.')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it.each([
    {
      refusal: new BackendError(400, {
        reason: 'invalid_credentials',
        detail: 'internal words',
        errors: [{ pointer: '/password', reason: 'invalid', detail: 'internal words' }],
      }),
      says: 'The current password is wrong.',
      at: 'Current password',
    },
    {
      refusal: new BackendError(400, {
        errors: [{ pointer: '/email', reason: 'invalid_format', detail: 'internal words' }],
      }),
      says: 'This is not an email address.',
      at: 'New email address',
    },
    {
      refusal: new BackendError(409, { reason: 'email_taken', detail: 'internal words' }),
      says: 'An account with that email address exists already.',
      at: 'New email address',
    },
    {
      refusal: new BackendError(429, { detail: 'internal words' }),
      says: 'Too many tries. Wait a moment and try again.',
      at: undefined,
    },
    { refusal: new Error('offline'), says: 'That did not work. Try again.', at: undefined },
  ])('says "$says" of a refusal, at its field, in its own words', async ({ refusal, says, at }) => {
    const { onRequest } = section({ onRequest: vi.fn(() => Promise.reject(refusal)) });
    await ask(NEW, 'a guess');
    expect(onRequest).toHaveBeenCalledOnce();
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(says);
    expect(alert).not.toHaveTextContent('internal words');
    // The form stays, with what was typed, and the focus goes to the field refused.
    expect(screen.getByLabelText('New email address')).toHaveValue(NEW);
    if (at) {
      expect(screen.getByLabelText(at)).toHaveFocus();
      expect(screen.getByLabelText(at)).toHaveAttribute('aria-invalid', 'true');
    }
  });

  it('says so of the address the account has already', async () => {
    const refusal = new BackendError(400, {
      errors: [{ pointer: '/email', reason: 'invalid', detail: '' }],
    });
    section({ onRequest: vi.fn(() => Promise.reject(refusal)) });
    await ask(OLD.toUpperCase(), 'the password');
    expect(await screen.findByRole('alert')).toHaveTextContent('This is your address already.');
  });

  it('says under the new address where the reset mail goes, or that a server without mail changes it at once', async () => {
    const { rerender } = section();
    await fireEvent.click(screen.getByRole('button', { name: 'Change address' }));
    const field = screen.getByLabelText('New email address');
    expect(field).toHaveAccessibleDescription(
      'Mail that resets your password goes to this address.',
    );
    await rerender({ mails: false });
    expect(field).toHaveAccessibleDescription(
      'This server sends no mail, so the address changes as soon as you ask.',
    );
  });

  it('tells an account with an address on a mailing server that a revert link lasts 7 days, and omits it otherwise', async () => {
    const { rerender } = section();
    const hint = screen.getByTestId('email-change-revert-hint');
    expect(hint).toHaveTextContent(
      'After a change, a link mailed to your previous address lets you take it back for 7 days.',
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Change address' }));
    expect(screen.getByTestId('email-change-revert-hint')).toBeInTheDocument();
    await rerender({ mails: false });
    expect(screen.queryByTestId('email-change-revert-hint')).toBeNull();
    await rerender({ mails: true, email: '' });
    expect(screen.queryByTestId('email-change-revert-hint')).toBeNull();
    await rerender({ mails: true, email: OLD, hasPassword: false });
    expect(screen.queryByTestId('email-change-revert-hint')).toBeNull();
  });

  it('offers no form to an account without a password, and says what comes first', () => {
    section({ hasPassword: false, email: '' });
    expect(screen.getByTestId('email-change-address')).toHaveTextContent('No address');
    expect(screen.queryByRole('button', { name: 'Change address' })).toBeNull();
    expect(screen.getByText(/Set one first/)).toBeInTheDocument();
  });
});

describe("the account's address, in the app", () => {
  async function hosted(change: Partial<Changes>, read: { email: string; pendingEmail?: string }) {
    const { workspace, backend, world } = setup();
    await workspace.load();
    const reader = world.users.get(backend.userId);
    if (!reader) throw new Error('no reader');
    // As the server answers `getMe`: the address, and the one the account waits for.
    const me = vi
      .spyOn(backend, 'me')
      .mockImplementation(() => Promise.resolve({ ...reader, hasPassword: true, ...read }));
    const loginOptions = vi.fn(() =>
      Promise.resolve({ registrationOpen: false, mailEnabled: true }),
    );
    const account = { emailChange: change, loginOptions } as Partial<Account> as Account;
    render(EmailChangeHost, { props: { workspace, account } });
    await settle();
    return { workspace, me, read };
  }

  it('reads the address anew as the section opens, and again once a change is asked for', async () => {
    const request = vi.fn<Changes['request']>(() => Promise.resolve());
    const read: { email: string; pendingEmail?: string } = { email: OLD };
    const { workspace, me } = await hosted({ request }, read);
    await waitFor(() => {
      expect(screen.getByTestId('email-change-address')).toHaveTextContent(OLD);
    });
    const before = me.mock.calls.length;
    read.pendingEmail = NEW;
    await ask(NEW, 'the password');
    expect(request).toHaveBeenCalledExactlyOnceWith(NEW, 'the password', 'en');
    expect(await screen.findByTestId('email-change-pending')).toHaveTextContent(NEW);
    expect(me.mock.calls.length).toBe(before + 1);
    expect(workspace.me).toMatchObject({ email: OLD, pendingEmail: NEW });
  });

  it('reads it again after a refusal too: the change was made in another browser meanwhile', async () => {
    const resend = vi.fn<Changes['resend']>(() => Promise.reject(new BackendError(404, {})));
    const read: { email: string; pendingEmail?: string } = { email: OLD, pendingEmail: NEW };
    const { workspace } = await hosted({ resend }, read);
    const again = await screen.findByRole('button', { name: 'Send the link again' });
    delete read.pendingEmail;
    read.email = NEW;
    await fireEvent.click(again);
    expect(await screen.findByText('No change is waiting any more.')).toBeInTheDocument();
    expect(screen.getByTestId('email-change-address')).toHaveTextContent(NEW);
    expect(screen.queryByTestId('email-change-pending')).toBeNull();
    expect(workspace.me?.pendingEmail).toBeUndefined();
  });
});

describe('the page of the link that changes the address', () => {
  it('is a route of its own under the deployment path', () => {
    expect(parseRoute('/boards/confirm-email', 'boards')).toEqual({ kind: 'emailConfirm' });
    expect(parseRoute('/confirm-email/secret', '').kind).toBe('unknown');
  });

  it('changes nothing until the button is pressed, then says the address is changed', async () => {
    const confirm = vi.fn((_token: string) => Promise.resolve());
    render(ConfirmEmailPage, { token: 'secret', confirm, homeHref: '/base' });
    expect(screen.getByText(/Opening the link changed nothing/)).toBeInTheDocument();
    expect(confirm).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: 'Confirm' }));
    expect(confirm).toHaveBeenCalledExactlyOnceWith('secret');
    expect(await screen.findByTestId('confirm-email-done')).toHaveTextContent(
      'Your address is changed.',
    );
    expect(screen.queryByRole('button', { name: 'Confirm' })).toBeNull();
    expect(screen.getByRole('link', { name: 'Continue' })).toHaveAttribute('href', '/base');
  });

  it('offers no button for a link without a token', () => {
    const confirm = vi.fn((_token: string) => Promise.resolve());
    render(ConfirmEmailPage, { token: '', confirm, homeHref: '/' });
    expect(screen.getByRole('alert')).toHaveTextContent('This link changes nothing.');
    expect(screen.queryByRole('button')).toBeNull();
    expect(confirm).not.toHaveBeenCalled();
  });

  it.each([
    {
      refusal: new BackendError(400, {
        errors: [{ pointer: '/token', reason: 'invalid', detail: 'internal words' }],
      }),
      says: 'This link changes nothing.',
      again: false,
    },
    {
      refusal: new BackendError(429, { detail: 'internal words' }),
      says: 'Too many tries. Wait a moment and try again.',
      again: true,
    },
  ])(
    'says "$says" of a link or a request that changed nothing',
    async ({ refusal, says, again }) => {
      const confirm = vi.fn((_token: string) => Promise.reject(refusal));
      render(ConfirmEmailPage, { token: 'secret', confirm, homeHref: '/' });
      await fireEvent.click(screen.getByRole('button', { name: 'Confirm' }));
      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(says);
      expect(alert).not.toHaveTextContent('internal words');
      expect(screen.queryByTestId('confirm-email-done')).toBeNull();
      // Only a request that failed is asked again; a refused link is not.
      expect(screen.queryByRole('button', { name: 'Confirm' }) !== null).toBe(again);
    },
  );
});

describe("the line about the address, in the reader's own space", () => {
  async function shown({
    own = true,
    hasPassword = true,
    kept = {},
    email = 'ada@work.example',
    mailEnabled = true,
  } = {}) {
    const setUp = setup();
    const { world, backend, workspace } = setUp;
    const reader = world.users.get(backend.userId);
    if (!reader) throw new Error('no reader');
    world.users.set(reader.id, { ...reader, hasPassword, email });
    world.screenState.set(reader.id, kept);
    const listed = await backend.workspaces();
    const at = listed.find((one) => (one.kind === 'personal') === own);
    if (!at) throw new Error('no such workspace');
    await backend.createBoard(at.id, { title: 'Mine' }, firstKanban([], 'Board'));
    await workspace.load(at.id);
    const loginOptions = vi.fn(() => Promise.resolve({ registrationOpen: false, mailEnabled }));
    const account = { loginOptions } as Partial<Account> as Account;
    render(PasswordNoticeHost, { props: { workspace, account, onProvider: vi.fn() } });
    await settle();
    return { ...setUp, readerId: reader.id };
  }

  it('stands in the own space of an account with a password, and is closed for good', async () => {
    const { world, readerId } = await shown();
    expect(await screen.findByTestId('email-notice')).toHaveTextContent(
      'If that is a company address, change it to a personal one in Settings',
    );
    expect(screen.queryByTestId('password-notice')).toBeNull();
    await fireEvent.click(screen.getByTestId('email-notice-dismiss'));
    expect(screen.queryByTestId('email-notice')).toBeNull();
    await waitFor(() => {
      expect(world.screenState.get(readerId)).toMatchObject({ emailNoticeDismissed: '1' });
    });
    expect(world.screenState.get(readerId)).not.toHaveProperty('passwordNoticeDismissed');
  });

  it('tells an account without an address that the mail has nowhere to go, and to add one', async () => {
    // As an account made through single sign-on is, once it has set a password.
    await shown({ email: '' });
    const line = await screen.findByTestId('email-notice');
    expect(line).toHaveTextContent('Your account has no email address');
    expect(line).toHaveTextContent('Add your own address in Settings');
    expect(line).not.toHaveTextContent('company address');
  });

  it('is not there in a shared workspace, once closed, beside the notice to set a password, or where the server sends no mail', async () => {
    for (const where of [
      { own: false },
      { kept: { emailNoticeDismissed: '1' } },
      { hasPassword: false },
      { mailEnabled: false },
    ]) {
      await shown(where);
      await settle();
      expect(screen.queryByTestId('email-notice')).toBeNull();
      cleanup();
    }
  });

  it('is shown to an account that closed the notice to set a password and set one later', async () => {
    await shown({ kept: { passwordNoticeDismissed: '1' } });
    expect(await screen.findByTestId('email-notice')).toBeInTheDocument();
  });
});
