/**
 * The page of the link that gives an account the address it had back: it
 * waits for its button, says what came of it, and leads to a reset link for
 * the address; and how it is asked of the server.
 */
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ServerEmailChange } from '$lib/api/email-change.ts';
import { Http } from '$lib/api/http.ts';
import RequestPasswordPage from '$lib/features/account/RequestPasswordPage.svelte';
import RevertEmailPage from '$lib/features/account/RevertEmailPage.svelte';
import { BackendError } from '$lib/state/index.ts';

import { emailOfForgotPassword, forgotPasswordHref } from '../../../src/app/recovery-links.ts';
import { parseRoute } from '../../../src/app/router.svelte.ts';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const OLD = 'ada+work@example.com';

function shown(revert: (token: string) => Promise<string | undefined>, token = 'secret') {
  const resetHref = (email: string) => forgotPasswordHref('/base/', email);
  render(RevertEmailPage, { token, revert, homeHref: '/base/', resetHref });
}

describe('the page of the link that gives the address back', () => {
  it('is a route of its own under the deployment path', () => {
    expect(parseRoute('/boards/revert-email', 'boards')).toEqual({ kind: 'emailRevert' });
    expect(parseRoute('/revert-email/secret', '').kind).toBe('unknown');
  });

  it('changes nothing until the button is pressed, then says the address is back and leads to a reset', async () => {
    const revert = vi.fn((_token: string) => Promise.resolve<string | undefined>(OLD));
    shown(revert);
    expect(screen.getByText(/Opening the link changed nothing/)).toBeInTheDocument();
    expect(revert).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: 'Take the address back' }));
    expect(revert).toHaveBeenCalledExactlyOnceWith('secret');
    const done = await screen.findByTestId('revert-email-done');
    expect(done).toHaveTextContent(`Your account has ${OLD} again, and is signed out everywhere.`);
    expect(done).toHaveTextContent('Its password works no more');
    const reset = screen.getByTestId('revert-email-reset');
    expect(reset).toHaveAttribute('href', '/base/forgot-password#email=ada%2Bwork%40example.com');
    expect(emailOfForgotPassword(new URL(reset.getAttribute('href') ?? '', 'http://x').hash)).toBe(
      OLD,
    );
    expect(screen.queryByRole('button', { name: 'Take the address back' })).toBeNull();
  });

  it('says the account address was cleared where another account has the one of the link, and directs to an administrator', async () => {
    shown(() => Promise.resolve(undefined));
    await fireEvent.click(screen.getByRole('button', { name: 'Take the address back' }));
    const done = await screen.findByTestId('revert-email-done');
    expect(done).toHaveTextContent(
      "Another account has this address now, so your account's email address was cleared, and your account is signed out everywhere.",
    );
    expect(done).toHaveTextContent(
      'Ask an administrator of your organization, or the operator of the server, for help.',
    );
    expect(screen.queryByTestId('revert-email-reset')).toBeNull();
  });

  it('offers no button for a link without a token', () => {
    const revert = vi.fn((_token: string) => Promise.resolve<string | undefined>(OLD));
    shown(revert, '');
    expect(screen.getByRole('alert')).toHaveTextContent('This link changes nothing.');
    expect(screen.queryByRole('button')).toBeNull();
    expect(revert).not.toHaveBeenCalled();
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
      shown(() => Promise.reject(refusal));
      await fireEvent.click(screen.getByRole('button', { name: 'Take the address back' }));
      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(says);
      expect(alert).not.toHaveTextContent('internal words');
      expect(screen.queryByTestId('revert-email-done')).toBeNull();
      expect(screen.queryByRole('button', { name: 'Take the address back' }) !== null).toBe(again);
    },
  );
});

describe('the page that asks for a reset link', () => {
  it('starts with the address it was led to with, which the reader may change', async () => {
    const request = vi.fn((_email: string) => Promise.resolve());
    render(RequestPasswordPage, {
      available: Promise.resolve(true),
      request,
      email: OLD,
      homeHref: '/',
    });
    const field = await screen.findByLabelText('Email address');
    expect(field).toHaveValue(OLD);
    await fireEvent.submit(screen.getByTestId('request-password-form'));
    expect(request).toHaveBeenCalledExactlyOnceWith(OLD);
  });

  it('reads no address from a fragment that has none', () => {
    expect(emailOfForgotPassword('')).toBe('');
    expect(emailOfForgotPassword('#token')).toBe('');
  });
});

it('asks the server with the token in the body, and answers the address given back', async () => {
  const sent: { url: string; body: unknown }[] = [];
  let answer: unknown = { email: OLD };
  vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
    sent.push({ url, body: typeof init.body === 'string' ? JSON.parse(init.body) : undefined });
    return Promise.resolve(
      new Response(JSON.stringify(answer), { headers: { 'Content-Type': 'application/json' } }),
    );
  });
  const change = new ServerEmailChange(new Http('/base'));
  expect(await change.revert('the token')).toBe(OLD);
  answer = {};
  expect(await change.revert('another')).toBeUndefined();
  expect(sent).toEqual([
    { url: '/base/api/v1/email-change/revert', body: { token: 'the token' } },
    { url: '/base/api/v1/email-change/revert', body: { token: 'another' } },
  ]);
});
