/**
 * The own space of an account with a password, to a session made through a
 * provider: it says why it does not open, asks for the password under the
 * account's user name, gives it, and says a refusal at the field.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import OwnSpaceLocked from '$lib/features/account/OwnSpaceLocked.svelte';
import { BackendError } from '$lib/state/index.ts';

afterEach(cleanup);

describe('the own space that opens with the password', () => {
  it('says why, names the account, and gives the password', async () => {
    const onConfirm = vi.fn(() => Promise.resolve());
    render(OwnSpaceLocked, { props: { username: 'grace', onConfirm } });
    const form = screen.getByRole('form', { name: 'My space opens with your password' });
    expect(form).toHaveTextContent('single sign-on alone does not open My space');
    expect(form).toHaveTextContent('You stay signed in as you are.');
    expect(screen.getByTestId('own-space-locked-user')).toHaveTextContent('User name: grace');
    // Nothing else had the focus: the field takes it.
    const field = screen.getByLabelText('Password');
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    await fireEvent.input(field, { target: { value: 'a long password' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Open My space' }));
    expect(onConfirm).toHaveBeenCalledExactlyOnceWith('a long password');
    // On its way into the space: the button waits.
    expect(screen.getByRole('button', { name: 'One moment…' })).toBeDisabled();
  });

  it('says a wrong password at the field, and may be tried again', async () => {
    const onConfirm = vi.fn(() =>
      Promise.reject(new BackendError(400, { reason: 'invalid_credentials', detail: 'wrong' })),
    );
    render(OwnSpaceLocked, { props: { username: 'grace', onConfirm } });
    await fireEvent.input(screen.getByLabelText('Password'), {
      target: { value: 'not it at all' },
    });
    const button = screen.getByRole('button', { name: 'Open My space' });
    button.focus();
    await fireEvent.click(button);
    await waitFor(() => {
      expect(screen.getByTestId('own-space-locked-problem')).toHaveTextContent(
        'The current password is wrong.',
      );
    });
    expect(screen.getByLabelText('Password')).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByRole('button', { name: 'Open My space' })).toBeEnabled();
    // The button was out of use meanwhile: the field has the focus again, to try again.
    await waitFor(() => {
      expect(screen.getByLabelText('Password')).toHaveFocus();
    });
  });

  it('leaves the focus where the reader put it', async () => {
    const elsewhere = document.createElement('button');
    document.body.append(elsewhere);
    elsewhere.focus();
    render(OwnSpaceLocked, { props: { username: 'grace', onConfirm: vi.fn() } });
    await Promise.resolve();
    expect(elsewhere).toHaveFocus();
    elsewhere.remove();
  });
});
