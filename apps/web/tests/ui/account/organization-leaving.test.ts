/**
 * Leaving an organization in the settings: the dialog says what leaving comes
 * to before anything is asked, an account without a password is told first
 * to set one, a refusal is said with what to do and leaves the dialog open,
 * and once left the page says so.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import OrganizationLeaving from '$lib/features/account/OrganizationLeaving.svelte';
import { BackendError } from '$lib/state/index.ts';

afterEach(cleanup);

const ORGANIZATIONS = [
  { orgId: 'a', name: 'Design' },
  { orgId: 'b', name: 'Sales' },
];

type Leave = (orgId: string) => Promise<void>;

function shown(onLeave = vi.fn<Leave>(() => Promise.resolve()), hasPassword = true) {
  render(OrganizationLeaving, { props: { organizations: ORGANIZATIONS, hasPassword, onLeave } });
  return onLeave;
}

async function asked(name: string): Promise<HTMLElement> {
  await fireEvent.click(screen.getByRole('button', { name: `Leave ${name}` }));
  return screen.findByRole('alertdialog');
}

describe('leaving an organization', () => {
  it('says what leaving comes to before anything is asked, and asks on the word', async () => {
    const onLeave = shown();
    const dialog = await asked('Sales');
    expect(dialog).toHaveTextContent('Leave Sales?');
    expect(dialog).toHaveTextContent('spaces, boards, wikis and courses, and of its groups');
    expect(dialog).toHaveTextContent('you wrote stay');
    expect(dialog).toHaveTextContent(
      'You can rejoin by invitation, or through SSO sign-in if the organization allows it.',
    );
    expect(within(dialog).queryByTestId('organization-leave-password-first')).toBeNull();
    expect(onLeave).not.toHaveBeenCalled();
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Leave' }));
    await waitFor(() => {
      expect(screen.queryByRole('alertdialog')).toBeNull();
    });
    expect(onLeave).toHaveBeenCalledExactlyOnceWith('b');
    expect(screen.getByRole('status')).toHaveTextContent('You left Sales.');
  });

  it('blocks an account without a password from leaving until one is set', async () => {
    const onLeave = shown(undefined, false);
    const dialog = await asked('Design');
    const first = within(dialog).getByTestId('organization-leave-password-first');
    expect(first).toHaveTextContent('Set a password before you can leave');
    const told = within(dialog).getAllByText(/./u, { selector: 'p, li' });
    expect(told.indexOf(first)).toBeLessThan(told.findIndex((one) => one.tagName === 'LI'));
    const leave = within(dialog).getByRole('button', { name: 'Leave' });
    expect(leave).toBeDisabled();
    await fireEvent.click(leave);
    expect(onLeave).not.toHaveBeenCalled();
  });

  it('handles a 409 password_required response from the server', async () => {
    const refusal = new BackendError(409, { reason: 'password_required' });
    const onLeave = shown(vi.fn<Leave>(() => Promise.reject(refusal)));
    const dialog = await asked('Design');
    const leave = within(dialog).getByRole('button', { name: 'Leave' });
    await fireEvent.click(leave);
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(
      'Set a password for your account before leaving',
    );
    expect(leave).toBeDisabled();
    expect(onLeave).toHaveBeenCalledOnce();
  });

  it('says a refusal with what to do, keeps the dialog, and does not ask again', async () => {
    const refusal = new BackendError(409, { reason: 'sole_org_admin' });
    const onLeave = shown(vi.fn<Leave>(() => Promise.reject(refusal)));
    const dialog = await asked('Design');
    const leave = within(dialog).getByRole('button', { name: 'Leave' });
    await fireEvent.click(leave);
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(
      'Make another member an administrator',
    );
    expect(screen.getByRole('alertdialog')).toBe(dialog);
    expect(leave).toBeDisabled();
    expect(onLeave).toHaveBeenCalledOnce();
    expect(screen.getByRole('button', { name: 'Leave Design' })).toBeInTheDocument();
  });

  it('lets a failure be tried again', async () => {
    const onLeave = vi
      .fn<Leave>()
      .mockRejectedValueOnce(new BackendError(0))
      .mockResolvedValueOnce(undefined);
    shown(onLeave);
    const dialog = await asked('Design');
    const leave = within(dialog).getByRole('button', { name: 'Leave' });
    await fireEvent.click(leave);
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Try again');
    expect(leave).toBeEnabled();
    await fireEvent.click(leave);
    await waitFor(() => {
      expect(onLeave).toHaveBeenCalledTimes(2);
    });
  });

  it('shows a load failure alert with a retry button when unread', async () => {
    const onRetry = vi.fn();
    render(OrganizationLeaving, {
      organizations: [],
      hasPassword: true,
      unread: true,
      onRetry,
      onLeave: vi.fn<Leave>(),
    });
    const section = screen.getByTestId('organization-leaving');
    expect(within(section).getByRole('alert')).toBeInTheDocument();
    await fireEvent.click(within(section).getByRole('button', { name: 'Try again' }));
    expect(onRetry).toHaveBeenCalledOnce();
  });
});
