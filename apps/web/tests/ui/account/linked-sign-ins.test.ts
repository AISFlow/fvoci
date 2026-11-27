/**
 * The connected sign-ins of the settings, the page of a workspace that asks
 * for single sign-on, and the notice of a sign-in that came back refused:
 * what each says, and what its buttons ask for.
 */
import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import LinkedSignIns from '$lib/features/account/LinkedSignIns.svelte';
import SignInNotice from '$lib/features/account/SignInNotice.svelte';
import SsoRequired from '$lib/features/account/SsoRequired.svelte';
import { m } from '$lib/i18n/paraglide/messages.js';
import { BackendError, type LinkedIdentity } from '$lib/state/index.ts';

afterEach(() => {
  cleanup();
});

const IDENTITY: LinkedIdentity = {
  issuer: 'https://login.example.com/realm',
  subject: 'there',
  email: 'ada@example.com',
  createdAt: 1,
  madeThisSession: false,
};

function mount(
  identities: LinkedIdentity[],
  refuse?: BackendError,
  organizations = [
    { orgId: 'org-a', name: 'Acme' },
    { orgId: 'org-b', name: 'Beta' },
  ],
) {
  const read = vi.fn(() => Promise.resolve(identities));
  const act = (): Promise<void> => (refuse ? Promise.reject(refuse) : Promise.resolve());
  const onLink = vi.fn((_orgId: string, _password: string) => act());
  const onUnlink = vi.fn((_identity: LinkedIdentity, _password: string) => act());
  render(LinkedSignIns, {
    props: { read, organizations: () => Promise.resolve(organizations), onLink, onUnlink },
  });
  return { read, onLink, onUnlink };
}

const password = (): HTMLElement => screen.getByLabelText('Current password');

describe('the connected sign-ins', () => {
  it('lists what is connected by the host of its provider and the address it gave', async () => {
    mount([IDENTITY]);
    const row = await screen.findByTestId('linked-sign-in');
    expect(row).toHaveTextContent('login.example.com');
    expect(row).toHaveTextContent('ada@example.com');
    expect(within(row).getByRole('button', { name: 'Disconnect' })).toBeInTheDocument();
  });

  it('connects through the one chosen, with the password, and stays busy as the browser leaves', async () => {
    const { onLink } = mount([]);
    expect(await screen.findByText('Nothing is connected yet.')).toBeInTheDocument();
    const opener = screen.getByRole('button', { name: 'Connect single sign-on' });
    await fireEvent.click(opener);
    expect(password()).toHaveFocus();
    await fireEvent.change(screen.getByRole('combobox'), { target: { value: 'org-b' } });
    await fireEvent.input(password(), { target: { value: 'the password' } });
    const go = screen.getByRole('button', { name: 'Continue to single sign-on' });
    await fireEvent.click(go);
    expect(onLink).toHaveBeenCalledExactlyOnceWith('org-b', 'the password');
    await vi.waitFor(() => {
      expect(screen.getByRole('button', { name: 'One moment…' })).toBeDisabled();
    });
  });

  it('closes the form with the button that opened it, and the focus goes back there', async () => {
    mount([]);
    const opener = await screen.findByRole('button', { name: 'Connect single sign-on' });
    await fireEvent.click(opener);
    await fireEvent.input(password(), { target: { value: 'typed' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByLabelText('Current password')).toBeNull();
    const again = screen.getByRole('button', { name: 'Connect single sign-on' });
    expect(again).toHaveFocus();
    // Opened again, nothing of what was typed is there.
    await fireEvent.click(again);
    expect(password()).toHaveValue('');
  });

  it('offers nothing to connect when no organization of the reader has a provider', async () => {
    mount([], undefined, []);
    expect(await screen.findByText(m.account_linked_none_offered())).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Connect single sign-on' })).toBeNull();
  });

  it('disconnects with the password, reads again, and says so', async () => {
    const { read, onUnlink } = mount([IDENTITY]);
    const row = await screen.findByTestId('linked-sign-in');
    await fireEvent.click(within(row).getByRole('button', { name: 'Disconnect' }));
    expect(password()).toHaveFocus();
    await fireEvent.input(password(), { target: { value: 'the password' } });
    read.mockResolvedValue([]);
    await fireEvent.click(screen.getByRole('button', { name: 'Disconnect it' }));
    expect(onUnlink).toHaveBeenCalledExactlyOnceWith(IDENTITY, 'the password');
    expect(await screen.findByText('Disconnected.')).toBeInTheDocument();
    expect(await screen.findByText('Nothing is connected yet.')).toBeInTheDocument();
    await vi.waitFor(() => {
      expect(screen.getByTestId('linked-sign-ins')).toHaveFocus();
    });
  });

  it('says before disconnecting the sign-in of this session that it signs the reader out', async () => {
    const { read, onUnlink } = mount([{ ...IDENTITY, madeThisSession: true }]);
    const row = await screen.findByTestId('linked-sign-in');
    await fireEvent.click(within(row).getByRole('button', { name: 'Disconnect' }));
    expect(screen.getByTestId('linked-ends-session')).toHaveTextContent(
      'disconnecting it signs you out',
    );
    await fireEvent.input(password(), { target: { value: 'the password' } });
    // The session is over: what reads next is refused, as the server refuses it.
    read.mockRejectedValue(new BackendError(401, {}));
    await fireEvent.click(screen.getByRole('button', { name: 'Disconnect it' }));
    expect(onUnlink).toHaveBeenCalledOnce();
    await vi.waitFor(() => {
      expect(read).toHaveBeenCalledTimes(2);
    });
  });

  it('says nothing of signing out for a sign-in this session was not made through', async () => {
    mount([IDENTITY]);
    const row = await screen.findByTestId('linked-sign-in');
    await fireEvent.click(within(row).getByRole('button', { name: 'Disconnect' }));
    expect(screen.queryByTestId('linked-ends-session')).toBeNull();
  });

  it.each([
    ['invalid_credentials', 400, 'The current password is wrong.'],
    ['last_sign_in', 409, 'This is the only way to sign in to this account, so it stays.'],
    ['password_not_set', 409, 'This account has no password to confirm this with.'],
  ] as const)('says why a disconnection was refused: %s', async (reason, status, words) => {
    mount([IDENTITY], new BackendError(status, { reason }));
    const row = await screen.findByTestId('linked-sign-in');
    await fireEvent.click(within(row).getByRole('button', { name: 'Disconnect' }));
    await fireEvent.input(password(), { target: { value: 'x' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Disconnect it' }));
    await vi.waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(words);
    });
    expect(screen.getByTestId('linked-sign-in')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Disconnect it' })).toBeEnabled();
  });

  it('says that it could not be read, and reads again when asked', async () => {
    const read = vi
      .fn<() => Promise<LinkedIdentity[]>>()
      .mockRejectedValueOnce(new BackendError(500))
      .mockResolvedValue([IDENTITY]);
    render(LinkedSignIns, {
      props: {
        read,
        organizations: () => Promise.resolve([]),
        onLink: vi.fn(() => Promise.resolve()),
        onUnlink: vi.fn(() => Promise.resolve()),
      },
    });
    expect(await screen.findByRole('alert')).toHaveTextContent(m.app_problem_load());
    expect(screen.queryByText('Nothing is connected yet.')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: m.app_retry() }));
    expect(await screen.findByTestId('linked-sign-in')).toBeInTheDocument();
  });
});

describe('a workspace that asks for single sign-on', () => {
  const OTHERS = [
    { id: 'w-open', title: 'Design', href: '/w/w-open' },
    { id: 'w-unnamed', title: '', href: '/w/w-unnamed' },
  ];

  function mount(start: () => Promise<void>, name = 'Marketing', others = OTHERS) {
    const onContinue = vi.fn(start);
    const onOpenLinked = vi.fn();
    const onGo = vi.fn();
    render(SsoRequired, { props: { name, others, onContinue, onOpenLinked, onGo } });
    return { onContinue, onOpenLinked, onGo };
  }

  it('says so by its name, goes to the provider, and stays busy as the browser leaves', async () => {
    const { onContinue, onOpenLinked } = mount(() => Promise.resolve());
    expect(screen.getByRole('heading')).toHaveTextContent('Marketing signs in with single sign-on');
    await fireEvent.click(screen.getByRole('button', { name: 'Connected sign-ins' }));
    expect(onOpenLinked).toHaveBeenCalledOnce();
    await fireEvent.click(screen.getByRole('button', { name: 'Continue with single sign-on' }));
    expect(onContinue).toHaveBeenCalledOnce();
    await vi.waitFor(() => {
      expect(screen.getByRole('button', { name: 'One moment…' })).toBeDisabled();
    });
  });

  it('is not on its way any more when the browser comes back to the page as it was left', async () => {
    mount(() => Promise.resolve());
    await fireEvent.click(screen.getByRole('button', { name: 'Continue with single sign-on' }));
    const shown = new Event('pageshow');
    Object.defineProperty(shown, 'persisted', { value: true });
    await fireEvent(window, shown);
    expect(screen.getByRole('button', { name: 'Continue with single sign-on' })).toBeEnabled();
  });

  it('lists the reader’s other workspaces, which is the way out', async () => {
    const { onGo } = mount(() => Promise.resolve());
    const links = within(screen.getByRole('navigation')).getAllByRole('link');
    expect(links.map((link) => link.getAttribute('href'))).toEqual(['/w/w-open', '/w/w-unnamed']);
    expect(links[0]).toHaveTextContent('Design');
    expect(links[1]).toHaveTextContent(m.account_sso_other_unnamed());
    const first = links[0];
    if (first) await fireEvent.click(first);
    expect(onGo).toHaveBeenCalledExactlyOnceWith('w-open');
  });

  it('has no list when no other is open', () => {
    mount(() => Promise.resolve(), '', []);
    expect(screen.getByRole('heading')).toHaveTextContent(m.account_sso_required_title());
    expect(screen.queryByRole('navigation')).toBeNull();
  });

  it('says that the start did not work, and can be tried again', async () => {
    mount(() => Promise.reject(new BackendError(409)));
    const button = screen.getByRole('button', { name: 'Continue with single sign-on' });
    await fireEvent.click(button);
    expect(await screen.findByRole('alert')).toHaveTextContent(m.account_oidc_failed());
    expect(button).toBeEnabled();
  });
});

describe('a sign-in that came back refused to somebody who is signed in', () => {
  it('says why, with the way to the connected sign-ins and the way on', async () => {
    const onGoOn = vi.fn();
    const onOpenLinked = vi.fn();
    render(SignInNotice, { props: { problem: 'oidc-identity-taken', onGoOn, onOpenLinked } });
    expect(screen.getByRole('alert')).toHaveTextContent(
      'That sign-in belongs to another account already. Nothing was connected.',
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Connected sign-ins' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Go on' }));
    expect(onOpenLinked).toHaveBeenCalledOnce();
    expect(onGoOn).toHaveBeenCalledOnce();
  });

  it('says that single sign-on knows them by another address than the account has', () => {
    render(SignInNotice, {
      props: { problem: 'oidc-email-mismatch', onGoOn: vi.fn(), onOpenLinked: vi.fn() },
    });
    expect(screen.getByRole('alert')).toHaveTextContent(
      "Single sign-on knows you by another email address than this account's. Nothing was connected.",
    );
  });

  it('does not tell somebody who is signed in to sign in with a password', () => {
    render(SignInNotice, {
      props: { problem: 'oidc-email-taken', onGoOn: vi.fn(), onOpenLinked: vi.fn() },
    });
    expect(screen.getByRole('alert')).toHaveTextContent(m.account_oidc_email_taken_signed_in());
  });
});
