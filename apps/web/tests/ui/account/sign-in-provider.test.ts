/**
 * The sign-in page and the organization's provider: the button that starts a
 * sign-in through it (a request of the page, not an address another page could
 * send a browser to), named after the organization, for the organization the
 * address names; and what a start or a sign-in that came back refused is told as.
 */
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import SignInPage from '$lib/features/account/SignInPage.svelte';
import { BackendError, type LoginOrganization } from '$lib/state/index.ts';

afterEach(() => {
  cleanup();
  history.replaceState(null, '', '/');
});

function mount(
  address: string,
  found: LoginOrganization | undefined,
  start: (orgId: string) => Promise<void> = () => Promise.resolve(),
) {
  history.replaceState(null, '', address);
  const organizationOf = vi.fn(async (_orgId: string | undefined) => ({
    ...(found ? { organization: found } : {}),
    registrationOpen: true,
  }));
  const onOidcStart = vi.fn(start);
  render(SignInPage, {
    props: {
      onSignIn: vi.fn(() => Promise.resolve()),
      onRegister: vi.fn(() => Promise.resolve('made' as const)),
      optionsOf: organizationOf,
      onOidcStart,
    },
  });
  return { organizationOf, onOidcStart };
}

describe('the provider of the organization on the sign-in page', () => {
  it('is a button named after the organization that starts a sign-in for it', async () => {
    const { organizationOf, onOidcStart } = mount('/login?org=org-1', {
      id: 'org-1',
      name: 'Acme',
      oidc: true,
    });
    const button = await screen.findByRole('button', { name: 'Continue with Acme' });
    expect(button).not.toHaveAttribute('href');
    expect(organizationOf).toHaveBeenCalledWith('org-1');
    await fireEvent.click(button);
    expect(onOidcStart).toHaveBeenCalledExactlyOnceWith('org-1');
  });

  it('says why a start was refused', async () => {
    mount('/login?org=org-1', { id: 'org-1', name: 'Acme', oidc: true }, () =>
      Promise.reject(new BackendError(409, { detail: 'no', reason: 'oidc_unavailable' })),
    );
    await fireEvent.click(await screen.findByRole('button', { name: 'Continue with Acme' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      "Signing in through single sign-on did not work. Try again, or ask your organization's administrator.",
    );
  });

  it('asks for the server’s only organization when the address names none, or an empty one', async () => {
    const { organizationOf } = mount('/login?org=', { id: 'only', name: '', oidc: true });
    expect(
      await screen.findByRole('button', { name: 'Continue with single sign-on' }),
    ).toBeInTheDocument();
    expect(organizationOf).toHaveBeenCalledWith(undefined);
  });

  it('is not there when the organization has no provider', async () => {
    const { organizationOf } = mount('/login?org=org-1', {
      id: 'org-1',
      name: 'Acme',
      oidc: false,
    });
    await vi.waitFor(() => {
      expect(organizationOf).toHaveBeenCalled();
    });
    // The answer read and the page drawn after it: then there is no button.
    await organizationOf.mock.results[0]?.value;
    await tick();
    await tick();
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeInTheDocument();
    expect(screen.queryByTestId('oidc-continue')).toBeNull();
  });

  it('tells a refusal by its name, and one it does not know in general words', () => {
    mount('/login?oidc_error=oidc-join-closed', undefined);
    expect(screen.getByRole('alert')).toHaveTextContent(
      'This organization does not let new people in through its single sign-on. Ask its administrator for an invitation.',
    );
    cleanup();
    mount('/login?oidc_error=<b>made-up</b>', undefined);
    expect(screen.getByRole('alert')).toHaveTextContent(
      "Signing in through single sign-on did not work. Try again, or ask your organization's administrator.",
    );
  });
});

describe('a sign-in held while the organization moves to another provider', () => {
  const acme: LoginOrganization = { id: 'org-1', name: 'Acme', oidc: true };

  function mountHeld(
    address: string,
    ends: { carryOver?: () => Promise<void>; startAnew?: () => Promise<void> } = {},
  ) {
    history.replaceState(null, '', address);
    const onCarryOver = vi.fn((_orgId: string) => (ends.carryOver ?? (() => Promise.resolve()))());
    const onStartAnew = vi.fn((_orgId: string) => (ends.startAnew ?? (() => Promise.resolve()))());
    render(SignInPage, {
      props: {
        onSignIn: vi.fn(() => Promise.resolve()),
        onRegister: vi.fn(() => Promise.resolve('made' as const)),
        optionsOf: () => Promise.resolve({ organization: acme, registrationOpen: false }),
        onOidcStart: vi.fn(() => Promise.resolve()),
        onCarryOver,
        onStartAnew,
      },
    });
    return { onCarryOver, onStartAnew };
  }

  it('asks whether the reader had an account, before any form', async () => {
    const { onCarryOver, onStartAnew } = mountHeld('/login?org=org-1&oidc_held=1');
    const choice = await screen.findByTestId('sign-in-choice');
    await vi.waitFor(() => {
      expect(choice).toHaveTextContent('Acme changed how its people sign in');
    });
    expect(screen.queryByTestId('sign-in-form')).not.toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('held-carry-over'));
    expect(onCarryOver).toHaveBeenCalledExactlyOnceWith('org-1');
    expect(onStartAnew).not.toHaveBeenCalled();
    // The browser is leaving for the previous provider: a second press starts nothing.
    expect(screen.getByTestId('held-carry-over')).toBeDisabled();
    expect(screen.getByTestId('held-start-anew')).toBeDisabled();
  });

  it('starts fresh in a new account when asked, and says why that was refused', async () => {
    const { onStartAnew } = mountHeld('/login?org=org-1&oidc_held=1', {
      startAnew: () => Promise.reject(new BackendError(409, { reason: 'oidc_join_closed' })),
    });
    await fireEvent.click(await screen.findByTestId('held-start-anew'));
    expect(onStartAnew).toHaveBeenCalledExactlyOnceWith('org-1');
    expect(await screen.findByRole('alert')).toHaveTextContent('Ask its administrator');
    expect(screen.getByTestId('held-start-anew')).toBeEnabled();
  });

  it('says that nothing is held any more', async () => {
    mountHeld('/login?org=org-1&oidc_held=1', {
      carryOver: () => Promise.reject(new BackendError(409, { reason: 'oidc_carry_over_refused' })),
    });
    await fireEvent.click(await screen.findByTestId('held-carry-over'));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'could not be continued with the previous sign-in',
    );
  });

  it('goes back to the sign-in form, and takes the held sign-in off the address', async () => {
    mountHeld('/login?org=org-1&oidc_held=1');
    await fireEvent.click(await screen.findByRole('button', { name: 'Back to sign-in' }));
    expect(await screen.findByTestId('sign-in-form')).toBeInTheDocument();
    expect(screen.queryByTestId('sign-in-choice')).not.toBeInTheDocument();
    expect(location.search).toBe('?org=org-1');
  });

  it('names the organization in the address it goes back to as one value', async () => {
    mountHeld('/login?org=a%26oidc_held%3D1&oidc_held=1');
    await fireEvent.click(await screen.findByRole('button', { name: 'Back to sign-in' }));
    expect(new URLSearchParams(location.search).get('org')).toBe('a&oidc_held=1');
    expect(new URLSearchParams(location.search).has('oidc_held')).toBe(false);
  });

  it('shows the form where the address names no organization, or holds nothing', async () => {
    mountHeld('/login?oidc_held=1');
    expect(await screen.findByTestId('sign-in-form')).toBeInTheDocument();
    cleanup();
    mountHeld('/login?org=org-1');
    expect(await screen.findByTestId('sign-in-form')).toBeInTheDocument();
    expect(screen.queryByTestId('sign-in-choice')).not.toBeInTheDocument();
  });

  it('tells a return from the previous provider that connected nothing', async () => {
    mountHeld('/login?org=org-1&oidc_error=oidc-carry-over-refused');
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'could not be continued with the previous sign-in',
    );
  });
});
