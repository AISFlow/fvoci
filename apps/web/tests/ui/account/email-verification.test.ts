/**
 * Where the server confirms addresses: the page that says to check the mail
 * after a registration, the refusal of a sign-in to a registration that waits,
 * and the page the mailed link opens.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import InvitePage from '$lib/features/account/InvitePage.svelte';
import SignInPage from '$lib/features/account/SignInPage.svelte';
import VerifyEmailPage from '$lib/features/account/VerifyEmailPage.svelte';
import { BackendError } from '$lib/state/index.ts';
import type { Confirmed } from '$lib/state/email-verification.ts';
import { parseRoute } from '../../../src/app/router.svelte.ts';

afterEach(cleanup);

async function fill(label: RegExp, value: string): Promise<void> {
  await fireEvent.input(screen.getByLabelText(label), { target: { value } });
}

function signInPage(onRegister: () => Promise<'made' | 'waiting'>) {
  const onSignIn = vi.fn(() => Promise.resolve());
  const onResend = vi.fn(() => Promise.resolve());
  render(SignInPage, {
    props: {
      onSignIn,
      onRegister: vi.fn(onRegister),
      onResend,
      optionsOf: () => Promise.resolve({ registrationOpen: true, emailVerification: true }),
    },
  });
  return { onSignIn, onResend };
}

describe('a registration that waits for its address', () => {
  it('says where the link went, sends it again and leads back to the form', async () => {
    const { onSignIn, onResend } = signInPage(() => Promise.resolve('waiting'));
    await fireEvent.click(
      await screen.findByRole('button', { name: 'New here? Create an account' }),
    );
    await fill(/^user name$/i, 'grace');
    await fill(/email/i, ' grace@example.test ');
    await fill(/^password$/i, 'a long password');
    await fireEvent.submit(screen.getByTestId('register-form'));

    const heading = await screen.findByRole('heading', { name: 'Check your email' });
    expect(heading).toHaveFocus();
    expect(screen.getByTestId('check-mail')).toHaveTextContent('grace@example.test');
    expect(onSignIn).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByRole('button', { name: 'Send the link again' }));
    await waitFor(() => {
      expect(screen.getByTestId('resend-status')).toHaveTextContent('a new link is on its way');
    });
    expect(onResend).toHaveBeenCalledExactlyOnceWith({ email: 'grace@example.test' });

    await fireEvent.click(screen.getByRole('button', { name: 'Sign up again' }));
    expect(await screen.findByTestId('register-form')).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByLabelText(/^user name$/i)).toHaveFocus();
    });
    expect(screen.getByTestId('verify-hint')).toHaveTextContent('We will mail a link');
  });

  it('refuses a sign-in to it with a way to the link, and keeps the wrong password vague', async () => {
    const onResend = vi.fn(() => Promise.resolve());
    const onSignIn = vi.fn(() =>
      Promise.reject(new BackendError(403, { reason: 'email_unverified', detail: 'not yet' })),
    );
    render(SignInPage, {
      props: {
        onSignIn,
        onRegister: vi.fn(() => Promise.resolve('made' as const)),
        onResend,
        optionsOf: () => Promise.resolve({ registrationOpen: true }),
      },
    });
    await fill(/user name or email/i, 'grace');
    await fill(/^password$/i, 'a long password');
    await fireEvent.submit(screen.getByTestId('sign-in-form'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Confirm your email address first');
    expect(screen.getByRole('alert')).not.toHaveTextContent('not yet');
    await fireEvent.click(screen.getByRole('button', { name: 'Send the link again' }));
    await waitFor(() => {
      expect(onResend).toHaveBeenCalledExactlyOnceWith({ username: 'grace', email: 'grace' });
    });
  });

  it('waits for the address of a new account made by an invitation', async () => {
    const onResend = vi.fn(() => Promise.resolve());
    const onJoined = vi.fn();
    const accept = vi.fn(() => Promise.resolve('waiting' as const));
    render(InvitePage, {
      props: {
        token: 'the-token',
        signedInAs: undefined,
        preview: () => Promise.resolve({ workspaceName: 'Acme', inviterName: 'Ada' }),
        accept,
        onResend,
        onSignIn: vi.fn(() => Promise.resolve()),
        onJoined,
        onSessionEnded: vi.fn(),
        onSso: vi.fn(() => Promise.resolve()),
        homeHref: '/',
      },
    });
    await screen.findByTestId('invite-summary');
    await fill(/^user name$/i, 'grace');
    await fill(/email/i, 'grace@example.test');
    await fill(/^password$/i, 'a long password');
    await fireEvent.click(screen.getByRole('button', { name: 'Create account and join' }));
    expect(await screen.findByTestId('check-mail')).toHaveTextContent('grace@example.test');
    expect(onJoined).not.toHaveBeenCalled();

    // The address was mistyped: back to the form, at the address, to accept again.
    expect(screen.getByTestId('check-mail')).toHaveTextContent('you keep your user name');
    await fireEvent.click(screen.getByRole('button', { name: 'Sign up again' }));
    await waitFor(() => {
      expect(screen.getByLabelText(/email/i)).toHaveFocus();
    });
    expect(screen.getByLabelText(/^user name$/i)).toHaveValue('grace');
    await fill(/email/i, 'grace@example.org');
    await fill(/^password$/i, 'a long password');
    await fireEvent.click(screen.getByRole('button', { name: 'Create account and join' }));
    expect(await screen.findByTestId('check-mail')).toHaveTextContent('grace@example.org');
    expect(accept).toHaveBeenCalledTimes(2);
  });
});

describe('the page of the confirmation link', () => {
  it('is a route of its own under the deployment path', () => {
    expect(parseRoute('/boards/verify-email', 'boards')).toEqual({ kind: 'emailVerify' });
    expect(parseRoute('/verify-email/secret', '').kind).toBe('unknown');
  });

  it('says the confirmation came through, sent once for the token', async () => {
    const complete = vi.fn((_token: string): Promise<Confirmed> =>
      Promise.resolve({ workspaceId: 'w1', organizationStarted: true }),
    );
    render(VerifyEmailPage, {
      confirming: complete('secret'),
      homeHref: '/',
      resend: vi.fn(),
      onPassword: vi.fn(),
    });
    await waitFor(() => {
      expect(screen.getByTestId('verify-status')).toHaveTextContent('Your address is confirmed');
    });
    expect(complete).toHaveBeenCalledExactlyOnceWith('secret');
  });

  it('says a link that confirms nothing is so, and mails a new one for the address', async () => {
    const complete = vi.fn(() =>
      Promise.reject(
        new BackendError(400, {
          errors: [{ pointer: '/token', reason: 'invalid', detail: 'internal value' }],
        }),
      ),
    );
    const resend = vi.fn(() => Promise.resolve());
    render(VerifyEmailPage, { confirming: complete(), homeHref: '/', resend, onPassword: vi.fn() });
    expect(await screen.findByTestId('verify-problem')).toHaveTextContent(
      'invalid, has expired or was replaced',
    );
    expect(screen.getByTestId('verify-problem')).not.toHaveTextContent('internal value');
    await fill(/email/i, ' grace@example.test ');
    await fireEvent.submit(screen.getByTestId('verify-request-form'));
    await waitFor(() => {
      expect(resend).toHaveBeenCalledExactlyOnceWith('grace@example.test');
    });
    expect(await screen.findByText(/a new link is on its way/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Back to sign in' })).toHaveAttribute('href', '/');
  });

  it('tells a request that got no answer from a link that confirms nothing', async () => {
    const failed = Promise.reject(new BackendError(0, { detail: 'no answer' }));
    render(VerifyEmailPage, {
      confirming: failed,
      homeHref: '/',
      resend: vi.fn(),
      onPassword: vi.fn(),
    });
    expect(await screen.findByTestId('verify-problem')).not.toHaveTextContent('invalid');
    expect(screen.queryByTestId('verify-request-form')).toBeNull();
  });

  it('says a link without a token confirms nothing', async () => {
    render(VerifyEmailPage, {
      confirming: undefined,
      homeHref: '/',
      resend: vi.fn(),
      onPassword: vi.fn(),
    });
    expect(await screen.findByTestId('verify-problem')).toHaveTextContent('invalid');
    expect(screen.getByTestId('verify-request-form')).toBeInTheDocument();
  });

  it('asks for the password of the registration where it was not made, and says a wrong one is', async () => {
    const asked = Promise.reject(
      new BackendError(403, { reason: 'password_required', detail: 'internal words' }),
    );
    const onPassword = vi.fn();
    const { rerender } = render(VerifyEmailPage, {
      confirming: asked,
      homeHref: '/',
      resend: vi.fn(),
      onPassword,
    });
    const field = await screen.findByLabelText('Password');
    expect(field).toHaveFocus();
    expect(field).toHaveAttribute('type', 'password');
    expect(screen.getByTestId('verify-password-intro')).toHaveTextContent(
      'not the browser you signed up in',
    );
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByTestId('verify-request-form')).toBeNull();

    await fireEvent.input(field, { target: { value: 'a guess' } });
    await fireEvent.submit(screen.getByTestId('verify-password-form'));
    expect(onPassword).toHaveBeenCalledExactlyOnceWith('a guess');

    // What the page is given next is that attempt: refused as a login is.
    await rerender({
      confirming: Promise.reject(
        new BackendError(401, { reason: 'invalid_credentials', detail: 'incorrect login' }),
      ),
    });
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'not the password this sign-up was made with',
    );
    expect(screen.getByLabelText('Password')).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByLabelText('Password')).toHaveValue('a guess');

    // Too many: told how it is, with the field still there to try again later.
    await rerender({
      confirming: Promise.reject(new BackendError(429, { detail: 'x', retryAfter: 30 })),
    });
    expect(await screen.findByRole('alert')).not.toHaveTextContent('not the password');
    expect(screen.getByTestId('verify-password-form')).toBeInTheDocument();

    // The link ended meanwhile: the password has nothing left to confirm.
    await rerender({
      confirming: Promise.reject(
        new BackendError(400, { errors: [{ pointer: '/token', reason: 'invalid', detail: 'x' }] }),
      ),
    });
    expect(await screen.findByTestId('verify-request-form')).toBeInTheDocument();
    expect(screen.queryByTestId('verify-password-form')).toBeNull();

    await rerender({
      confirming: Promise.resolve({ workspaceId: 'w1', organizationStarted: false }),
    });
    await waitFor(() => {
      expect(screen.getByTestId('verify-status')).toHaveTextContent('Your address is confirmed');
    });
  });

  it('says what became of a registration that a right password found gone, without a form', async () => {
    const onPassword = vi.fn();
    const { rerender } = render(VerifyEmailPage, {
      confirming: Promise.reject(new BackendError(403, { reason: 'password_required' })),
      homeHref: '/',
      resend: vi.fn(),
      onPassword,
    });
    await fireEvent.input(await screen.findByLabelText('Password'), {
      target: { value: 'the password' },
    });
    await fireEvent.submit(screen.getByTestId('verify-password-form'));
    // The password was right: the registration's link lets nobody in, and it is removed.
    await rerender({
      confirming: Promise.reject(
        new BackendError(404, { reason: 'invitation_unavailable', detail: 'x' }),
      ),
    });
    expect(await screen.findByTestId('verify-problem')).toHaveTextContent(
      'This invite link cannot be used',
    );
    expect(screen.queryByTestId('verify-password-form')).toBeNull();
    expect(screen.queryByTestId('verify-request-form')).toBeNull();
  });

  it('says an invitation that lets nobody in any more is so, without a form', async () => {
    const complete = vi.fn(() =>
      Promise.reject(new BackendError(404, { reason: 'invitation_unavailable', detail: 'x' })),
    );
    render(VerifyEmailPage, {
      confirming: complete(),
      homeHref: '/',
      resend: vi.fn(),
      onPassword: vi.fn(),
    });
    expect(await screen.findByTestId('verify-problem')).toHaveTextContent(
      'This invite link cannot be used',
    );
    expect(screen.queryByTestId('verify-request-form')).toBeNull();
  });
});
