/**
 * The page of an invitation: what it says of the link, and how whoever reads
 * it gets in (signed in: a press; signed out: an account to make, or the one
 * they have). And the sign-in page of a server that takes nobody without one.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { m } from '$lib/i18n/paraglide/messages.js';
import { setLocale } from '$lib/i18n/paraglide/runtime.js';

import InvitePage from '$lib/features/account/InvitePage.svelte';
import SignInPage from '$lib/features/account/SignInPage.svelte';
import {
  BackendError,
  type AcceptedInvitation,
  type InvitationPreview,
  type NewAccount,
} from '$lib/state/index.ts';

afterEach(cleanup);

const GONE = new BackendError(404, { detail: 'no', reason: 'invitation_unavailable' });
const SSO_REQUIRED = new BackendError(401, { reason: 'sso_required', orgId: 'org-a' });

function mount(options: {
  preview?: () => Promise<InvitationPreview>;
  accept?: (token: string, account?: NewAccount) => Promise<AcceptedInvitation>;
  signedInAs?: string;
  onSignIn?: (login: string, password: string) => Promise<void>;
  onSso?: (orgId: string) => Promise<void>;
}) {
  const accept = vi.fn(options.accept ?? (() => Promise.resolve({ workspaceId: 'w1' })));
  const onSignIn = vi.fn(options.onSignIn ?? (() => Promise.resolve()));
  const onJoined = vi.fn();
  const onSessionEnded = vi.fn();
  const onSso = vi.fn(options.onSso ?? (() => Promise.resolve()));
  render(InvitePage, {
    props: {
      token: 'the-token',
      signedInAs: options.signedInAs,
      preview:
        options.preview ?? (() => Promise.resolve({ workspaceName: 'Acme', inviterName: 'Ada' })),
      accept,
      onSignIn,
      onJoined,
      onSessionEnded,
      onSso,
      homeHref: '/',
    },
  });
  return { accept, onSignIn, onJoined, onSessionEnded, onSso };
}

async function fill(label: RegExp | string, value: string): Promise<void> {
  await fireEvent.input(screen.getByLabelText(label), { target: { value } });
}

describe('the page of an invitation', () => {
  it('says who invites and where to, and lets in whoever is signed in with one press', async () => {
    const { accept, onSignIn, onJoined } = mount({ signedInAs: 'Grace (@grace)' });
    expect(await screen.findByTestId('invite-summary')).toHaveTextContent(
      'Ada invited you to Acme',
    );
    expect(screen.getByText('You are signed in as Grace (@grace).')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Join' }));
    await waitFor(() => {
      expect(onJoined).toHaveBeenCalledExactlyOnceWith({ workspaceId: 'w1' });
    });
    expect(accept).toHaveBeenCalledExactlyOnceWith('the-token');
    expect(onSignIn).not.toHaveBeenCalled();
  });

  it('names the board a guest is invited to', async () => {
    mount({
      preview: () =>
        Promise.resolve({ workspaceName: 'Acme', inviterName: 'Ada', containerTitle: 'Roadmap' }),
    });
    expect(await screen.findByTestId('invite-summary')).toHaveTextContent(
      'Ada invited you to the board “Roadmap” of Acme',
    );
  });

  it('names the organization, never a workspace, when the workspace has no name, in both languages', async () => {
    const cases = [
      { locale: 'en', board: undefined, want: 'Ada invited you to their organization' },
      {
        locale: 'en',
        board: 'Roadmap',
        want: 'Ada invited you to the board “Roadmap” of their organization',
      },
      { locale: 'ko', board: undefined, want: 'Ada 님이 조직에 초대했습니다' },
      { locale: 'ko', board: 'Roadmap', want: 'Ada 님이 조직의 보드 “Roadmap”에 초대했습니다' },
    ] as const;
    try {
      for (const { locale, board, want } of cases) {
        void setLocale(locale, { reload: false });
        mount({
          preview: () =>
            Promise.resolve({
              workspaceName: '',
              inviterName: 'Ada',
              ...(board === undefined ? {} : { containerTitle: board }),
            }),
        });
        const summary = await screen.findByTestId('invite-summary');
        expect(summary.textContent.trim()).toBe(want);
        expect(screen.getByTestId('invite-page').textContent).not.toMatch(
          /workspace|워크스페이스/i,
        );
        cleanup();
      }
    } finally {
      void setLocale('en', { reload: false });
    }
  });

  it('makes an account for whoever has none, lets it in, and signs it in', async () => {
    const order: string[] = [];
    const { accept, onSignIn, onJoined } = mount({
      accept: () => {
        order.push('accept');
        return Promise.resolve({ workspaceId: 'w1', containerId: 'b1' });
      },
      onSignIn: () => {
        order.push('signIn');
        return Promise.resolve();
      },
    });
    await screen.findByTestId('invite-register-form');
    await fill(/^user name$/i, ' grace ');
    await fill(/^display name$/i, 'Grace Hopper');
    await fill(/email/i, ' grace@example.test ');
    await fill(/^password$/i, 'a long password');
    await fireEvent.click(screen.getByRole('button', { name: 'Create account and join' }));
    await waitFor(() => {
      expect(onJoined).toHaveBeenCalledExactlyOnceWith({ workspaceId: 'w1', containerId: 'b1' });
    });
    expect(accept).toHaveBeenCalledExactlyOnceWith('the-token', {
      username: 'grace',
      displayName: 'Grace Hopper',
      email: 'grace@example.test',
      password: 'a long password',
    });
    expect(onSignIn).toHaveBeenCalledExactlyOnceWith('grace', 'a long password');
    expect(order).toEqual(['accept', 'signIn']);
  });

  it('says what is wrong with an account beside its fields', async () => {
    const { onJoined } = mount({
      accept: () =>
        Promise.reject(
          new BackendError(409, {
            detail: 'taken',
            reason: 'username_taken',
            errors: [{ pointer: '/account/username', detail: 'taken', reason: 'username_taken' }],
          }),
        ),
    });
    await screen.findByTestId('invite-register-form');
    await fill(/^user name$/i, 'grace');
    await fill(/email/i, 'grace@example.test');
    await fill(/^password$/i, 'a long password');
    await fireEvent.click(screen.getByRole('button', { name: 'Create account and join' }));
    expect(await screen.findByTestId('problem-username')).toHaveTextContent(
      'That user name is taken.',
    );
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(onJoined).not.toHaveBeenCalled();
  });

  it('signs in whoever has an account, and lets that in', async () => {
    const { accept, onSignIn, onJoined } = mount({});
    await screen.findByTestId('invite-register-form');
    await fireEvent.click(screen.getByRole('button', { name: 'Have an account? Sign in' }));
    await fill(/user name or email/i, 'grace');
    await fill(/^password$/i, 'a long password');
    await fireEvent.click(screen.getByRole('button', { name: 'Sign in and join' }));
    await waitFor(() => {
      expect(onJoined).toHaveBeenCalledExactlyOnceWith({ workspaceId: 'w1' });
    });
    expect(onSignIn).toHaveBeenCalledExactlyOnceWith('grace', 'a long password');
    expect(accept).toHaveBeenCalledExactlyOnceWith('the-token');
  });

  it('says of a link that lets nobody in only that, with no form', async () => {
    mount({ preview: () => Promise.reject(GONE) });
    expect(await screen.findByTestId('invite-unavailable')).toHaveTextContent(
      'This invite link cannot be used',
    );
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Go to the first page' })).toHaveAttribute('href', '/');
  });

  it('says so too when the link stops letting anybody in while the page is open', async () => {
    const { onJoined } = mount({ signedInAs: 'grace', accept: () => Promise.reject(GONE) });
    await fireEvent.click(await screen.findByRole('button', { name: 'Join' }));
    expect(await screen.findByTestId('invite-unavailable')).toBeInTheDocument();
    expect(onJoined).not.toHaveBeenCalled();
  });

  it('starts again, signed out, when the session ended before the press', async () => {
    const { onJoined, onSessionEnded } = mount({
      signedInAs: 'grace',
      accept: () => Promise.reject(new BackendError(401, { detail: 'no session' })),
    });
    await fireEvent.click(await screen.findByRole('button', { name: 'Join' }));
    await waitFor(() => {
      expect(onSessionEnded).toHaveBeenCalledOnce();
    });
    expect(onJoined).not.toHaveBeenCalled();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('sends whoever is signed in another way through the single sign-on of an organization that takes no password', async () => {
    const { onJoined, onSessionEnded, onSso } = mount({
      signedInAs: 'grace',
      accept: () => Promise.reject(SSO_REQUIRED),
    });
    await fireEvent.click(await screen.findByRole('button', { name: 'Join' }));
    expect(await screen.findByTestId('invite-sso')).toHaveTextContent(
      'opens only for its own single sign-on, and you are signed in another way',
    );
    expect(onSessionEnded).not.toHaveBeenCalled();
    expect(onJoined).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: 'Continue with single sign-on' }));
    expect(onSso).toHaveBeenCalledExactlyOnceWith('org-a');
  });

  it('tells a good password apart from the single sign-on an organization asks for', async () => {
    const { onSignIn, onSso } = mount({ accept: () => Promise.reject(SSO_REQUIRED) });
    await fireEvent.click(await screen.findByRole('button', { name: 'Have an account? Sign in' }));
    await fill(/User name or email address/, 'grace');
    await fill(/^Password/, 'the password');
    await fireEvent.click(screen.getByRole('button', { name: 'Sign in and join' }));
    expect(await screen.findByTestId('invite-sso')).toBeInTheDocument();
    expect(onSignIn).toHaveBeenCalledExactlyOnceWith('grace', 'the password');
    expect(screen.queryByText(/name or password/i)).not.toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Continue with single sign-on' }));
    expect(onSso).toHaveBeenCalledExactlyOnceWith('org-a');
  });

  it('offers to try again when the link could not be read at all', async () => {
    mount({ preview: () => Promise.reject(new Error('offline')) });
    expect(await screen.findByRole('button', { name: 'Try again' })).toBeInTheDocument();
    expect(screen.queryByTestId('invite-unavailable')).not.toBeInTheDocument();
  });
});

describe('the sign-in page and who may make an account', () => {
  function signInPage(registrationOpen: boolean) {
    const onRegister = vi.fn(() => Promise.resolve('made' as const));
    const onSignIn = vi.fn(() => Promise.resolve());
    render(SignInPage, {
      props: { onSignIn, onRegister, optionsOf: () => Promise.resolve({ registrationOpen }) },
    });
    return { onRegister, onSignIn };
  }

  it('makes an account on its own by default, which names no organization', async () => {
    const { onRegister, onSignIn } = signInPage(true);
    await fireEvent.click(
      await screen.findByRole('button', { name: 'New here? Create an account' }),
    );
    expect(await screen.findByTestId('register-solo')).toBeChecked();
    expect(screen.queryByLabelText(m.account_organization_name())).toBeNull();
    await fill(/^user name$/i, 'ada');
    await fill(/email/i, 'ada@example.test');
    await fill(/^password$/i, 'a long password');
    await fireEvent.click(screen.getByRole('button', { name: 'Create account' }));
    await waitFor(() => {
      expect(onSignIn).toHaveBeenCalledExactlyOnceWith('ada', 'a long password');
    });
    expect(onRegister).toHaveBeenCalledExactlyOnceWith({
      username: 'ada',
      displayName: '',
      email: 'ada@example.test',
      password: 'a long password',
    });
  });

  it('asks for the name of what is started when the reader works together with others', async () => {
    const { onRegister } = signInPage(true);
    await fireEvent.click(
      await screen.findByRole('button', { name: 'New here? Create an account' }),
    );
    await fireEvent.click(await screen.findByTestId('register-together'));
    await fill(m.account_organization_name(), ' Acme ');
    await fill(/^user name$/i, 'ada');
    await fill(/email/i, 'ada@example.test');
    await fill(/^password$/i, 'a long password');
    await fireEvent.click(screen.getByRole('button', { name: 'Create account' }));
    await waitFor(() => {
      expect(onRegister).toHaveBeenCalledExactlyOnceWith({
        organizationName: 'Acme',
        username: 'ada',
        displayName: '',
        email: 'ada@example.test',
        password: 'a long password',
      });
    });
  });

  it('sends nothing with a blank name where one was asked for, and says so at the field', async () => {
    const { onRegister } = signInPage(true);
    await fireEvent.click(
      await screen.findByRole('button', { name: 'New here? Create an account' }),
    );
    await fireEvent.click(await screen.findByTestId('register-together'));
    await fill(m.account_organization_name(), '   ');
    await fill(/^user name$/i, 'ada');
    await fill(/email/i, 'ada@example.test');
    await fill(/^password$/i, 'a long password');
    await fireEvent.submit(screen.getByTestId('register-form'));
    expect(await screen.findByTestId('problem-organizationName')).toHaveTextContent(
      'Fill this in.',
    );
    await waitFor(() => {
      expect(screen.getByLabelText(m.account_organization_name())).toHaveFocus();
    });
    expect(onRegister).not.toHaveBeenCalled();
  });

  it('asks the first account of the server for the name, with no choice, and says why', async () => {
    const onRegister = vi.fn(() => Promise.resolve('made' as const));
    render(SignInPage, {
      props: {
        onSignIn: vi.fn(() => Promise.resolve()),
        onRegister,
        optionsOf: () => Promise.resolve({ registrationOpen: true, organizationRequired: true }),
      },
    });
    await fireEvent.click(
      await screen.findByRole('button', { name: 'New here? Create an account' }),
    );
    expect(await screen.findByTestId('register-first')).toHaveTextContent(
      'The first account of this server creates its',
    );
    expect(screen.queryByTestId('register-use')).toBeNull();
    await fill(m.account_organization_name(), 'Acme');
    await fill(/^user name$/i, 'ada');
    await fill(/email/i, 'ada@example.test');
    await fill(/^password$/i, 'a long password');
    await fireEvent.click(screen.getByRole('button', { name: 'Create account' }));
    await waitFor(() => {
      expect(onRegister).toHaveBeenCalledExactlyOnceWith(
        expect.objectContaining({ organizationName: 'Acme', username: 'ada' }),
      );
    });
  });

  it('leaves a blank name of the first account to the server, which tells it with all else wrong', async () => {
    const onRegister = vi.fn(() => Promise.resolve('made' as const));
    render(SignInPage, {
      props: {
        onSignIn: vi.fn(() => Promise.resolve()),
        onRegister,
        optionsOf: () => Promise.resolve({ registrationOpen: true, organizationRequired: true }),
      },
    });
    await fireEvent.click(
      await screen.findByRole('button', { name: 'New here? Create an account' }),
    );
    await screen.findByTestId('register-first');
    await fill(/^user name$/i, 'ada');
    await fill(/email/i, 'ada@example.test');
    await fill(/^password$/i, 'a long password');
    await fireEvent.submit(screen.getByTestId('register-form'));
    await waitFor(() => {
      expect(onRegister).toHaveBeenCalledExactlyOnceWith(
        expect.objectContaining({ organizationName: '', username: 'ada' }),
      );
    });
  });

  it('offers no account where the server has its organization, and says what to ask for', async () => {
    signInPage(false);
    expect(await screen.findByTestId('invitation-needed')).toHaveTextContent(
      'No account yet? Ask an administrator of your organization for an invite link.',
    );
    expect(screen.queryByRole('button', { name: /create an account/i })).not.toBeInTheDocument();
  });
});
