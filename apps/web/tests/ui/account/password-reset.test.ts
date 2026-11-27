import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';

import ResetPasswordPage from '$lib/features/account/ResetPasswordPage.svelte';
import { BackendError } from '$lib/state/index.ts';
import { parseRoute } from '../../../src/app/router.svelte.ts';

afterEach(() => {
  cleanup();
  history.replaceState(null, '', '/');
});

it('recognizes the reset page under the deployment path', () => {
  expect(parseRoute('/boards/reset-password', 'boards')).toEqual({ kind: 'passwordReset' });
  expect(parseRoute('/reset-password/secret', '').kind).toBe('unknown');
});

it('sets the new password, forgets the secret and offers sign in', async () => {
  history.replaceState(null, '', '/reset-password#secret');
  const complete = vi.fn(() => Promise.resolve());
  render(ResetPasswordPage, { token: 'secret', homeHref: '/', complete });
  await fireEvent.input(screen.getByLabelText('New password'), {
    target: { value: 'New-password' },
  });
  await fireEvent.submit(screen.getByTestId('reset-password-form'));
  expect(complete).toHaveBeenCalledExactlyOnceWith('secret', 'New-password');
  expect(await screen.findByRole('status')).toHaveTextContent('Your password has been reset');
  expect(location.hash).toBe('');
  expect(screen.getByRole('link', { name: 'Back to sign in' })).toHaveAttribute('href', '/');
  expect(screen.queryByLabelText('New password')).toBeNull();
});

it('explains an unavailable link without showing the server detail', async () => {
  const complete = vi.fn(() =>
    Promise.reject(
      new BackendError(400, {
        errors: [{ pointer: '/token', reason: 'invalid', detail: 'internal value' }],
      }),
    ),
  );
  render(ResetPasswordPage, { token: 'expired', homeHref: '/', complete });
  await fireEvent.input(screen.getByLabelText('New password'), {
    target: { value: 'New-password' },
  });
  await fireEvent.submit(screen.getByTestId('reset-password-form'));
  expect(await screen.findByRole('alert')).toHaveTextContent('invalid or has expired');
  expect(screen.getByRole('alert')).not.toHaveTextContent('internal value');
});

it('offers no email form when SMTP is unavailable', async () => {
  const { default: RequestPasswordPage } =
    await import('$lib/features/account/RequestPasswordPage.svelte');
  render(RequestPasswordPage, {
    available: Promise.resolve(false),
    request: vi.fn(),
    homeHref: '/',
  });
  expect(await screen.findByText(/Contact your server administrator/)).toHaveAttribute(
    'role',
    'status',
  );
  expect(screen.queryByLabelText('Email address')).toBeNull();
});

it('acknowledges a request without claiming the account exists', async () => {
  const { default: RequestPasswordPage } =
    await import('$lib/features/account/RequestPasswordPage.svelte');
  const request = vi.fn(() => Promise.resolve());
  render(RequestPasswordPage, { available: Promise.resolve(true), request, homeHref: '/' });
  await fireEvent.input(await screen.findByLabelText('Email address'), {
    target: { value: 'absent@example.test' },
  });
  await fireEvent.submit(screen.getByTestId('request-password-form'));
  expect(request).toHaveBeenCalledExactlyOnceWith('absent@example.test');
  expect(await screen.findByRole('status')).toHaveTextContent(
    'If this email belongs to an account',
  );
});

it('shows the forgot-password link only when the server can send mail', async () => {
  const { default: SignInPage } = await import('$lib/features/account/SignInPage.svelte');
  const props = { onSignIn: vi.fn(), onRegister: vi.fn(), forgotHref: '/boards/forgot-password' };
  const view = render(SignInPage, {
    ...props,
    optionsOf: () => Promise.resolve({ registrationOpen: false, mailEnabled: false }),
  });
  await screen.findByTestId('invitation-needed');
  expect(screen.queryByRole('link', { name: 'Forgot your password?' })).toBeNull();
  view.unmount();
  render(SignInPage, {
    ...props,
    optionsOf: () => Promise.resolve({ registrationOpen: false, mailEnabled: true }),
  });
  expect(await screen.findByRole('link', { name: 'Forgot your password?' })).toHaveAttribute(
    'href',
    '/boards/forgot-password',
  );
});
