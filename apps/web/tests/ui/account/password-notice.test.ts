/**
 * The notice that asks an account without a password to set one: why, set
 * from the notice itself, a session too old led through the provider again,
 * a refused password told at its field, and closed for good.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import PasswordNotice from '$lib/features/account/PasswordNotice.svelte';
import { BackendError } from '$lib/state/index.ts';

afterEach(cleanup);

function mount(onSet: (password: string) => Promise<void>) {
  const props = {
    username: 'grace',
    onSet: vi.fn(onSet),
    onSignInAgain: vi.fn(() => Promise.resolve()),
    onDismiss: vi.fn(),
    onClose: vi.fn(),
  };
  render(PasswordNotice, { props });
  return props;
}

async function setPassword(password: string): Promise<void> {
  await fireEvent.input(screen.getByLabelText('New password'), { target: { value: password } });
  await fireEvent.click(screen.getByRole('button', { name: 'Set password' }));
}

describe('the notice to set a password', () => {
  it('says why, sets it, and then says how to sign in', async () => {
    const props = mount(() => Promise.resolve());
    const notice = screen.getByRole('region', { name: 'Set a password' });
    expect(notice).toHaveTextContent('nothing else opens My space');
    expect(notice).toHaveTextContent('My space asks for it once after each single sign-on');
    // It takes no focus of its own.
    expect(notice).not.toContainElement(document.activeElement as HTMLElement | null);
    await setPassword('a long password');
    expect(props.onSet).toHaveBeenCalledExactlyOnceWith('a long password');
    const done = await screen.findByTestId('password-notice-done');
    expect(done).toHaveTextContent('Your user name is grace');
    const close = screen.getByRole('button', { name: 'Close' });
    await waitFor(() => {
      expect(close).toHaveFocus();
    });
    await fireEvent.click(close);
    expect(props.onClose).toHaveBeenCalledOnce();
    expect(props.onDismiss).not.toHaveBeenCalled();
  });

  it('leads a session too old to vouch through the provider again', async () => {
    const props = mount(() =>
      Promise.reject(new BackendError(403, { reason: 'sign_in_again', detail: 'again' })),
    );
    expect(screen.queryByTestId('password-notice-provider')).toBeNull();
    await setPassword('a long password');
    expect(await screen.findByTestId('password-notice-problem')).toHaveTextContent(
      'Sign in through single sign-on again, then set it within five minutes.',
    );
    await fireEvent.click(screen.getByTestId('password-notice-provider'));
    expect(props.onSignInAgain).toHaveBeenCalledOnce();
  });

  it('tells a refused password at its field', async () => {
    mount(() =>
      Promise.reject(
        new BackendError(400, {
          errors: [{ pointer: '/password', reason: 'too_short', detail: 'short' }],
        }),
      ),
    );
    await setPassword('short');
    const field = screen.getByLabelText('New password');
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    expect(field).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByTestId('password-notice-problem')).toHaveTextContent(
      'The new password needs at least 8 characters.',
    );
  });

  it('says so of an account that has a password already, and has nothing left to ask', async () => {
    mount(() => Promise.reject(new BackendError(409, { detail: 'has one' })));
    await setPassword('a long password');
    expect(await screen.findByTestId('password-notice-done')).toHaveTextContent(
      'This account has a password already.',
    );
    expect(screen.queryByLabelText('New password')).toBeNull();
    // The form gave way: the focus is on what is left to do.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
    });
  });

  it('closes for good with its own button', async () => {
    const props = mount(() => Promise.resolve());
    await fireEvent.click(screen.getByRole('button', { name: "Don't show again" }));
    expect(props.onDismiss).toHaveBeenCalledOnce();
    expect(props.onSet).not.toHaveBeenCalled();
  });
});
