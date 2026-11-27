/** SSO settings preserve secrets and explain the server's lockout guards. */
import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import OrgSso from '$lib/features/organization/OrgSso.svelte';
import { BackendError } from '$lib/state/backend.ts';
import type { OidcProvider, OidcSettings } from '$lib/state/oidc.ts';

afterEach(cleanup);
const provider: OidcProvider = {
  issuer: 'https://login.example.test',
  clientId: 'client',
  hasClientSecret: true,
  enabled: true,
  autoJoin: true,
  ssoRequired: false,
  canRequireSso: false,
  redirectUri: 'https://app.example.test/api/v1/auth/oidc/callback/org',
  createdAt: 1,
  updatedAt: 1,
  pending: null,
  former: null,
};

function mount(value: OidcProvider | undefined = provider) {
  const save = vi.fn(async (settings: OidcSettings): Promise<OidcProvider> => ({
    ...provider,
    ...settings,
  }));
  const remove = vi.fn(() => Promise.resolve());
  const onSignIn = vi.fn(() => Promise.resolve());
  const discard = vi.fn(() => Promise.resolve());
  const onTry = vi.fn(() => Promise.resolve());
  const endFormer = vi.fn((_createdAt: number) => Promise.resolve());
  const read = vi.fn(() => Promise.resolve(value));
  render(OrgSso, { read, save, remove, onSignIn, discard, onTry, endFormer });
  return { read, save, remove, onSignIn, discard, onTry, endFormer };
}

describe('the SSO settings', () => {
  it('keeps a stored secret out of the form and unchanged on save', async () => {
    const { save } = mount();
    await screen.findByText('Client secret is stored.');
    expect(screen.queryByTestId('sso-secret')).not.toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('sso-save'));
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledOnce();
    });
    expect(save.mock.calls[0]?.[0]).not.toHaveProperty('clientSecret');
    expect(screen.getByTestId('sso-callback')).toHaveValue(provider.redirectUri);
  });

  it('only enables enforcement for the current provider and session', async () => {
    const { onSignIn } = mount();
    await screen.findByTestId('sso-client');
    expect(screen.getByTestId('sso-required-switch')).toBeDisabled();
    await fireEvent.click(screen.getByTestId('sso-sign-in'));
    expect(onSignIn).toHaveBeenCalledOnce();
  });

  it('says that another issuer signs out who signed in through the current one', async () => {
    mount();
    const issuer = await screen.findByTestId('sso-issuer');
    expect(screen.queryByTestId('sso-issuer-change')).not.toBeInTheDocument();
    await fireEvent.input(issuer, { target: { value: 'https://other.example.test' } });
    expect(screen.getByTestId('sso-issuer-change')).toHaveTextContent('signs out everyone');
    await fireEvent.input(issuer, { target: { value: provider.issuer } });
    expect(screen.queryByTestId('sso-issuer-change')).not.toBeInTheDocument();
  });

  it('asks for the secret of another issuer, and can keep the stored one otherwise', async () => {
    const { save } = mount();
    const issuer = await screen.findByTestId('sso-issuer');
    await fireEvent.input(issuer, { target: { value: ' https://other.example.test ' } });
    expect(screen.getByTestId('sso-secret')).toBeInTheDocument();
    expect(screen.queryByTestId('sso-keep-secret')).not.toBeInTheDocument();
    await fireEvent.input(screen.getByTestId('sso-secret'), { target: { value: 'new secret' } });
    await fireEvent.click(screen.getByTestId('sso-save'));
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledOnce();
    });
    expect(save.mock.calls[0]?.[0]).toMatchObject({
      issuer: 'https://other.example.test',
      clientSecret: 'new secret',
    });
  });

  it('goes back to the stored secret after asking to change it', async () => {
    const { save } = mount();
    await fireEvent.click(await screen.findByRole('button', { name: 'Change secret' }));
    expect(screen.getByTestId('sso-secret')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('sso-keep-secret'));
    expect(screen.queryByTestId('sso-secret')).not.toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('sso-save'));
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledOnce();
    });
    expect(save.mock.calls[0]?.[0]).not.toHaveProperty('clientSecret');
  });

  it('changing the client requires proving the replacement too', async () => {
    mount({ ...provider, canRequireSso: true });
    const client = await screen.findByTestId('sso-client');
    expect(screen.getByTestId('sso-required-switch')).toBeEnabled();
    await fireEvent.input(client, { target: { value: 'other-client' } });
    expect(screen.getByTestId('sso-required-switch')).toBeDisabled();
  });

  it('turns the requirement off while a new secret is typed, and sends what it shows', async () => {
    const { save } = mount({ ...provider, canRequireSso: true });
    const required = await screen.findByTestId('sso-required-switch');
    await fireEvent.click(required);
    expect(required).toBeChecked();
    await fireEvent.click(screen.getByText('Change secret'));
    expect(required).not.toBeChecked();
    expect(required).toBeDisabled();
    expect(screen.getByTestId('sso-secret-unproven')).toBeInTheDocument();
    await fireEvent.input(screen.getByTestId('sso-secret'), { target: { value: 'replacement' } });
    await fireEvent.click(screen.getByTestId('sso-save'));
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledOnce();
    });
    expect(save.mock.calls[0]?.[0]).toMatchObject({
      clientSecret: 'replacement',
      ssoRequired: false,
    });
  });

  it('does not disable or delete a provider while enforcement is stored', async () => {
    mount({ ...provider, canRequireSso: true, ssoRequired: true });
    await screen.findByTestId('sso-client');
    for (const id of ['sso-enabled', 'sso-delete']) {
      expect(screen.getByTestId(id)).toBeDisabled();
    }
    for (const id of ['sso-issuer', 'sso-client', 'sso-required-switch']) {
      expect(screen.getByTestId(id)).toBeEnabled();
    }
    expect(screen.getByTestId('sso-pending-hint')).toBeInTheDocument();
    // Another issuer only waits aside: nobody is signed out by saving it.
    await fireEvent.input(screen.getByTestId('sso-issuer'), {
      target: { value: 'https://next.example.com' },
    });
    expect(screen.queryByTestId('sso-issuer-change')).not.toBeInTheDocument();
    // Switched off in the same save, it is taken at once, and signs people out.
    await fireEvent.click(screen.getByTestId('sso-required-switch'));
    expect(screen.getByTestId('sso-issuer-change')).toHaveTextContent('signs out everyone');
  });

  it('says a change saved while enforced waits aside, and tries or discards it', async () => {
    const enforced = { ...provider, canRequireSso: true, ssoRequired: true };
    const pending = {
      issuer: provider.issuer,
      clientId: 'next-client',
      hasClientSecret: true,
      createdAt: 2,
      bindsIdentityUntil: null,
    };
    const { save, onTry } = mount(enforced);
    save.mockResolvedValueOnce({ ...enforced, pending });
    const client = await screen.findByTestId('sso-client');
    await fireEvent.input(client, { target: { value: 'next-client' } });
    await fireEvent.input(screen.getByTestId('sso-secret'), { target: { value: 'next secret' } });
    await fireEvent.click(screen.getByTestId('sso-save'));
    await screen.findByText(/Saved as pending/);
    expect(save.mock.calls[0]?.[0]).toMatchObject({ clientId: 'next-client', ssoRequired: true });
    expect(screen.getByTestId('sso-client')).toHaveValue(provider.clientId);
    expect(screen.getByTestId('sso-pending-client')).toHaveTextContent('next-client');
    expect(document.body.textContent).not.toContain('next secret');
    await fireEvent.click(screen.getByTestId('sso-pending-try'));
    expect(onTry).toHaveBeenCalledOnce();
  });

  it('throws the pending settings away', async () => {
    const pending = {
      issuer: provider.issuer,
      clientId: 'c',
      hasClientSecret: false,
      createdAt: 2,
      bindsIdentityUntil: null,
    };
    const { discard } = mount({ ...provider, ssoRequired: true, pending });
    await fireEvent.click(await screen.findByTestId('sso-pending-discard'));
    await vi.waitFor(() => {
      expect(discard).toHaveBeenCalledOnce();
    });
    await screen.findByText('The pending settings were discarded.');
  });

  it('shows the previous provider until its day, and ends it early after asking', async () => {
    const former = {
      issuer: 'https://before.example.test',
      createdAt: Date.UTC(2026, 10, 1),
      expiresAt: Date.UTC(2026, 11, 1, 12),
    };
    const { read, endFormer } = mount({ ...provider, former });
    expect(await screen.findByTestId('sso-former-issuer')).toHaveTextContent(former.issuer);
    // The moment in the reader's zone: the second of December east of UTC+12.
    const moment = new Intl.DateTimeFormat('en', {
      dateStyle: 'long',
      timeStyle: 'short',
    }).format(former.expiresAt);
    expect(moment).toMatch(/^December [12], 2026 at \d+:00/);
    expect(screen.getByTestId('sso-former-until')).toHaveTextContent(moment);
    await fireEvent.click(screen.getByTestId('sso-former-end'));
    expect(endFormer).not.toHaveBeenCalled();
    read.mockResolvedValueOnce(provider);
    const dialog = await screen.findByTestId('confirm-dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'End early' }));
    await screen.findByText("The previous provider's linking period was ended.");
    expect(endFormer).toHaveBeenCalledExactlyOnceWith(former.createdAt);
    expect(screen.queryByTestId('sso-former')).not.toBeInTheDocument();
  });

  it('reads the provider again when ending is refused: another one may be kept by now', async () => {
    const former = { issuer: 'https://before.example.test', createdAt: 1, expiresAt: 2 };
    const next = { issuer: 'https://between.example.test', createdAt: 3, expiresAt: 4 };
    const { read, endFormer } = mount({ ...provider, former });
    endFormer.mockRejectedValueOnce(new BackendError(409, {}));
    await fireEvent.click(await screen.findByTestId('sso-former-end'));
    read.mockResolvedValueOnce({ ...provider, former: next });
    const dialog = await screen.findByTestId('confirm-dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'End early' }));
    await vi.waitFor(() => {
      expect(screen.getByTestId('sso-former-issuer')).toHaveTextContent(next.issuer);
    });
    expect(screen.getByRole('alert')).not.toBeEmptyDOMElement();
  });

  it('shows no previous provider where none is kept', async () => {
    mount();
    await screen.findByTestId('sso-client');
    expect(screen.queryByTestId('sso-former')).not.toBeInTheDocument();
  });

  it('asks to sign in again before trying settings of another issuer from an older session', async () => {
    const enforced = { ...provider, canRequireSso: true, ssoRequired: true };
    const moving = {
      issuer: 'https://next.example.test',
      clientId: 'client',
      hasClientSecret: true,
      createdAt: 2,
      // Passed, by the server's clock: it answers none.
      bindsIdentityUntil: null,
    };
    const { onSignIn, onTry } = mount({ ...enforced, pending: moving });
    expect(await screen.findByTestId('sso-pending-binding')).toHaveTextContent(
      'sign in through the current provider again first',
    );
    await fireEvent.click(screen.getByTestId('sso-pending-sign-in-again'));
    expect(onSignIn).toHaveBeenCalledOnce();
    expect(onTry).not.toHaveBeenCalled();
    // An identity connected already still applies them: trying stays possible.
    expect(screen.getByTestId('sso-pending-try')).toBeInTheDocument();
  });

  it('offers no sign-in again to a session that did not come through the provider', async () => {
    // The requirement released, settings of another issuer still aside, a password session.
    const moving = {
      issuer: 'https://next.example.test',
      clientId: 'client',
      hasClientSecret: true,
      createdAt: 2,
      bindsIdentityUntil: null,
    };
    mount({ ...provider, pending: moving });
    await screen.findByTestId('sso-pending-binding');
    expect(screen.queryByTestId('sso-pending-sign-in-again')).not.toBeInTheDocument();
    expect(screen.getByTestId('sso-pending-try')).toBeInTheDocument();
  });

  it('says until when trying settings of another issuer connects the new identity', async () => {
    const enforced = { ...provider, canRequireSso: true, ssoRequired: true };
    const until = Date.now() + 4 * 60 * 1000;
    const moving = {
      issuer: 'https://next.example.test',
      clientId: 'client',
      hasClientSecret: true,
      createdAt: 2,
      bindsIdentityUntil: until,
    };
    const { onTry } = mount({ ...enforced, pending: moving });
    const time = new Intl.DateTimeFormat('en', { timeStyle: 'short' }).format(until);
    expect(await screen.findByTestId('sso-pending-binding')).toHaveTextContent(`by ${time}`);
    expect(screen.queryByTestId('sso-pending-sign-in-again')).not.toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('sso-pending-try'));
    expect(onTry).toHaveBeenCalledOnce();
  });

  it('says nothing of connecting an identity for settings of the same issuer', async () => {
    const pending = {
      issuer: provider.issuer,
      clientId: 'c',
      hasClientSecret: false,
      createdAt: 2,
      bindsIdentityUntil: null,
    };
    mount({ ...provider, ssoRequired: true, pending });
    await screen.findByTestId('sso-pending');
    expect(screen.queryByTestId('sso-pending-binding')).not.toBeInTheDocument();
    expect(screen.queryByTestId('sso-pending-sign-in-again')).not.toBeInTheDocument();
  });

  it('says to sign in again when a trial came back too late to connect the identity', async () => {
    history.replaceState(null, '', '/org/o?oidc_pending_error=oidc-trial-stale');
    mount();
    await screen.findByText(/Sign in through the current provider again, then finish/);
    expect(location.search).toBe('');
  });

  it('tells what came of a sign-in through the pending settings, once', async () => {
    history.replaceState(null, '', '/org/o?oidc_pending_error=oidc-token-refused');
    mount();
    await screen.findByText(/did not accept the client ID or secret/);
    expect(location.search).toBe('');
    cleanup();
    history.replaceState(null, '', '/org/o?oidc_pending=applied');
    mount();
    await screen.findByText(/They are now in use/);
    expect(location.search).toBe('');
  });

  it('keeps an entered secret for retry and clears it after saving', async () => {
    const { save } = mount();
    save.mockRejectedValueOnce(new BackendError(400, { reason: 'oidc_issuer_unverified' }));
    await screen.findByText('Client secret is stored.');
    await fireEvent.click(screen.getByText('Change secret'));
    await fireEvent.input(screen.getByTestId('sso-secret'), { target: { value: 'replacement' } });
    await fireEvent.click(screen.getByTestId('sso-save'));
    await screen.findByText(/The issuer's discovery could not be verified/);
    expect(save.mock.calls[0]?.[0].clientSecret).toBe('replacement');
    expect(screen.getByTestId('sso-secret')).toHaveValue('replacement');
    await fireEvent.click(screen.getByTestId('sso-save'));
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledTimes(2);
    });
    expect(save.mock.calls[1]?.[0].clientSecret).toBe('replacement');
    await vi.waitFor(() => {
      expect(screen.queryByTestId('sso-secret')).not.toBeInTheDocument();
    });
    expect(document.body.textContent).not.toContain('replacement');
  });
});
