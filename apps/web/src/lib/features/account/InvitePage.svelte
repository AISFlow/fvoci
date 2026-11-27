<!--
  The page of an invitation: what the link of an invitation opens
  (`/invite/<token>`).

  It says who invites and where to, and lets in whoever reads it: somebody
  signed in joins with a press; somebody who is not makes an account, or
  signs in with the one they have, and joins with that. A link that lets
  nobody in is said to be so in one way, whatever is wrong with it: the
  server tells no more.

  Making an account does not sign in on the server, so the page signs in
  right after with what was typed. A refusal is told as `problems.ts` says.
  An organization that takes no password lets in no session but one of its
  single sign-on: the page says so and offers the way through it, which comes back
  to this page.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    isBackendError,
    isSignedOut,
    ssoOrgOf,
    type AcceptedInvitation,
    type InvitationPreview,
    type NewAccount,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import type { WaitingAccount } from '$lib/state/email-verification.ts';
  import { forgetInviteToken } from '$lib/fragment-token.ts';
  import AccountFields from './AccountFields.svelte';
  import CheckMailPage from './CheckMailPage.svelte';
  import ResendLink from './ResendLink.svelte';
  import {
    accountFieldsUnder,
    formProblemsOf,
    type AccountField,
    type FieldProblems,
  } from './problems.ts';

  interface Props {
    /** The token of the link, from the address. */
    token: string;
    /** The name of whoever is signed in; undefined for nobody. */
    signedInAs: string | undefined;
    preview: (token: string) => Promise<InvitationPreview>;
    /** Lets in, or makes a new account wait for its address to be confirmed (`waiting`). */
    accept: (token: string, account?: NewAccount) => Promise<AcceptedInvitation | 'waiting'>;
    /** Mails a new account that waits for its address a new link. */
    onResend?: (waiting: WaitingAccount) => Promise<void>;
    onSignIn: (login: string, password: string) => Promise<void>;
    /** Shows where the invitation let the reader in. */
    onJoined: (accepted: AcceptedInvitation) => void;
    /**
     * Whoever was signed in no longer is (the session ended while the page was
     * open): the page is to be shown again, to nobody.
     */
    onSessionEnded: () => void;
    /** Starts a sign-in through the organization's single sign-on, to come back here. */
    onSso: (orgId: string) => Promise<void>;
    /** The address of the app's first page, for a link that lets nobody in. */
    homeHref: string;
  }

  let {
    token,
    signedInAs,
    preview,
    accept,
    onResend,
    onSignIn,
    onJoined,
    onSessionEnded,
    onSso,
    homeHref,
  }: Props = $props();

  /** What the link shows; `undefined` for one that lets nobody in. */
  const shown = $derived(
    token === ''
      ? Promise.resolve({ found: undefined, failed: false })
      : preview(token).then(
          (found) => ({ found, failed: false }),
          (error: unknown) => {
            const unavailable = isBackendError(error) && error.reason === 'invitation_unavailable';
            if (unavailable) forgetInviteToken();
            return {
              found: undefined,
              // Anything but "no such link" may pass: the page offers to try again.
              failed: !unavailable,
            };
          },
        ),
  );

  let mode = $state<'register' | 'signIn'>('register');
  let login = $state('');
  let username = $state('');
  let displayName = $state('');
  let email = $state('');
  let password = $state('');
  let working = $state(false);
  let problem = $state<string | undefined>(undefined);
  let fieldProblems = $state.raw<FieldProblems>({});
  /** The link stopped letting anybody in while the page was open. */
  let gone = $state(false);
  /** Signed in on this page with an account they had: joining is one more step, taken at once. */
  let signedIn = $state(false);
  /** The name or address a sign-in was refused for, as not confirmed yet. */
  let unconfirmed = $state<string | undefined>(undefined);
  /** The address a new account's link went to, while it waits for it. */
  let waitingFor = $state<string | undefined>(undefined);
  /** The organization of the link takes no session but one of its single sign-on. */
  let ssoOrg = $state<string | undefined>(undefined);

  /** The field of each form by the member of its request, a JSON Pointer. */
  const FIELDS: Record<'register' | 'signIn', Record<string, AccountField>> = {
    register: accountFieldsUnder('/account'),
    signIn: { '/username': 'login', '/email': 'login', '/password': 'password' },
  };

  async function run(
    form: 'register' | 'signIn' | undefined,
    step: () => Promise<void>,
  ): Promise<void> {
    if (working) return;
    working = true;
    problem = undefined;
    unconfirmed = undefined;
    fieldProblems = {};
    try {
      await step();
    } catch (error) {
      if (form === 'signIn' && isBackendError(error) && error.reason === 'email_unverified') {
        unconfirmed = login.trim();
      }
      if (isBackendError(error) && error.reason === 'invitation_unavailable') {
        forgetInviteToken();
        gone = true;
        return;
      }
      // Not the name or the password: the session is good, and not one it takes.
      ssoOrg = ssoOrgOf(error);
      if (ssoOrg !== undefined) return;
      const said = formProblemsOf(error, form ? FIELDS[form] : {});
      fieldProblems = said.fields;
      problem = said.problem;
      if (said.first !== undefined) {
        await tick();
        document.getElementById(`account-${said.first}`)?.focus();
      }
    } finally {
      working = false;
    }
  }

  /** Shows where an account that has one was let in. */
  function letIn(accepted: AcceptedInvitation | 'waiting'): void {
    // Who is signed in is let in at once: only a new account waits.
    if (accepted !== 'waiting') {
      forgetInviteToken();
      onJoined(accepted);
    }
  }

  /** Joins as whoever is signed in. */
  function join(): Promise<void> {
    return run(undefined, async () => {
      try {
        letIn(await accept(token));
      } catch (error) {
        // The session ended meanwhile: there is nobody to join as, and nothing
        // on this form of the page to sign in with.
        if (isSignedOut(error)) onSessionEnded();
        else throw error;
      }
    });
  }

  function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (mode === 'register') {
      return run('register', async () => {
        const name = username.trim();
        const accepted = await accept(token, {
          username: name,
          // Trimmed by the server, as it counts white space.
          displayName,
          email: email.trim(),
          password,
        });
        if (accepted === 'waiting') {
          password = '';
          waitingFor = email.trim();
          return;
        }
        // The account is there and let in. Should signing in fail, the form
        // to go on with is the one to sign in with, the name filled in.
        login = name;
        mode = 'signIn';
        await onSignIn(name, password);
        forgetInviteToken();
        onJoined(accepted);
      });
    }
    return run('signIn', async () => {
      if (!signedIn) await onSignIn(login.trim(), password);
      signedIn = true;
      letIn(await accept(token));
    });
  }

  /** To the single sign-on of `orgId`: the browser leaves, the button as it is. */
  async function toSso(orgId: string): Promise<void> {
    if (working) return;
    working = true;
    problem = undefined;
    try {
      await onSso(orgId);
    } catch {
      problem = m.account_oidc_failed();
      working = false;
    }
  }

  function switchTo(next: 'register' | 'signIn'): void {
    mode = next;
    signedIn = false;
    problem = undefined;
    fieldProblems = {};
  }

  /**
   * Back from waiting to the form, to register again by the link with the
   * address put right: this browser's new registration replaces the one that
   * waits, under the same name. At the address.
   */
  async function signUpAgain(): Promise<void> {
    waitingFor = undefined;
    switchTo('register');
    await tick();
    document.getElementById('account-email')?.focus();
  }
</script>

{#snippet invitedTo(found: InvitationPreview)}
  <p class="text-lg font-medium" data-testid="invite-summary">
    {#if found.containerTitle !== undefined}
      {(found.containerKind === 'wiki'
        ? m.invite_summary_wiki
        : found.containerKind === 'course'
          ? m.invite_summary_course
          : m.invite_summary_board)({
        inviter: found.inviterName,
        workspace: found.workspaceName || m.invite_workspace_unnamed(),
        board: found.containerTitle || m.invite_board_untitled(),
      })}
    {:else}
      {m.invite_summary({
        inviter: found.inviterName,
        workspace: found.workspaceName || m.invite_workspace_unnamed(),
      })}
    {/if}
  </p>
{/snippet}

{#snippet fieldProblem(field: 'login' | 'password')}
  {#if fieldProblems[field]}
    <p
      id="account-{field}-problem"
      class="text-sm text-destructive"
      role="alert"
      data-testid="problem-{field}"
    >
      {fieldProblems[field]}
    </p>
  {/if}
{/snippet}

{#snippet unavailable()}
  <div class="grid gap-3 text-center" data-testid="invite-unavailable">
    <h1 class="text-lg font-medium">{m.invite_unavailable_title()}</h1>
    <p class="text-sm text-muted-foreground">{m.invite_unavailable_hint()}</p>
    <Button href={homeHref} variant="outline" class="roomy:min-h-touch">
      {m.invite_to_app()}
    </Button>
  </div>
{/snippet}

<!-- Back from the provider by the browser's "back", the page is as it was left: usable again. -->
<svelte:window
  onpageshow={(event: PageTransitionEvent) => {
    if (event.persisted) working = false;
  }}
/>

{#if waitingFor !== undefined}
  {@const address = waitingFor}
  <CheckMailPage
    email={address}
    onResend={() => onResend?.({ email: address }) ?? Promise.resolve()}
    onSignUpAgain={() => void signUpAgain()}
  />
{:else}
  <main class="grid min-h-dvh place-items-center p-4">
    <div class="grid w-full max-w-sm gap-4" data-testid="invite-page">
      <p class="text-center text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
      {#await shown}
        <p class="text-center text-muted-foreground" role="status">{m.app_loading()}</p>
      {:then { found, failed }}
        {#if gone || (!found && !failed)}
          {@render unavailable()}
        {:else if !found}
          <p class="text-center" role="alert">{m.invite_load_failed()}</p>
          <Button
            class="roomy:min-h-touch"
            onclick={() => {
              location.reload();
            }}
          >
            {m.app_retry()}
          </Button>
        {:else if ssoOrg !== undefined}
          {@const orgId = ssoOrg}
          <div class="grid gap-4 text-center" data-testid="invite-sso">
            {@render invitedTo(found)}
            <p class="text-sm text-muted-foreground">{m.account_sso_required_text()}</p>
            <!-- In the page from the start: a live region that appears with its words is often not read. -->
            <p class="text-sm text-destructive empty:hidden" role="alert">{problem ?? ''}</p>
            <Button
              class="roomy:min-h-touch"
              disabled={working}
              onclick={() => void toSso(orgId)}
              data-testid="invite-sso-continue"
            >
              {working ? m.account_working() : m.account_sso_continue()}
            </Button>
          </div>
        {:else if signedInAs !== undefined}
          <div class="grid gap-4 text-center">
            {@render invitedTo(found)}
            <p class="text-sm text-muted-foreground">
              {m.invite_joining_as({ name: signedInAs })}
            </p>
            {#if problem}
              <p class="text-sm text-destructive" role="alert">{problem}</p>
            {/if}
            <Button
              class="roomy:min-h-touch"
              disabled={working}
              onclick={() => void join()}
              data-testid="invite-join"
            >
              {working ? m.account_working() : m.invite_join()}
            </Button>
          </div>
        {:else}
          <form
            class="grid gap-4"
            onsubmit={(event) => void submit(event)}
            data-testid={mode === 'register' ? 'invite-register-form' : 'invite-sign-in-form'}
          >
            <div class="text-center">
              {@render invitedTo(found)}
              <h1 class="mt-2 text-sm text-muted-foreground">
                {mode === 'register' ? m.invite_register_title() : m.invite_sign_in_title()}
              </h1>
            </div>

            {#if mode === 'register'}
              <AccountFields
                bind:username
                bind:displayName
                bind:email
                bind:password
                problems={fieldProblems}
              />
            {:else}
              <div class="grid gap-1.5">
                <Label for="account-login">{m.account_login()}</Label>
                <Input
                  id="account-login"
                  name="login"
                  bind:value={login}
                  oninput={() => {
                    signedIn = false;
                  }}
                  autocomplete="username"
                  required
                  aria-invalid={fieldProblems.login ? 'true' : undefined}
                  aria-describedby={fieldProblems.login ? 'account-login-problem' : undefined}
                  class="roomy:h-touch roomy:text-base"
                />
                {@render fieldProblem('login')}
              </div>
              <div class="grid gap-1.5">
                <Label for="account-password">{m.account_password()}</Label>
                <Input
                  id="account-password"
                  name="password"
                  type="password"
                  bind:value={password}
                  oninput={() => {
                    signedIn = false;
                  }}
                  autocomplete="current-password"
                  required
                  aria-invalid={fieldProblems.password ? 'true' : undefined}
                  aria-describedby={fieldProblems.password ? 'account-password-problem' : undefined}
                  class="roomy:h-touch roomy:text-base"
                />
                {@render fieldProblem('password')}
              </div>
            {/if}

            {#if problem}
              <p class="text-sm text-destructive" role="alert">{problem}</p>
            {/if}
            {#if unconfirmed !== undefined && onResend}
              {@const named = unconfirmed}
              <ResendLink onResend={() => onResend({ username: named, email: named })} />
            {/if}

            <Button type="submit" class="roomy:min-h-touch" disabled={working}>
              {#if working}
                {m.account_working()}
              {:else}
                {mode === 'register' ? m.invite_register() : m.invite_sign_in()}
              {/if}
            </Button>

            {#if !signedIn}
              <Button
                variant="link"
                class="roomy:min-h-touch"
                disabled={working}
                onclick={() => {
                  switchTo(mode === 'register' ? 'signIn' : 'register');
                }}
              >
                {mode === 'register' ? m.invite_to_sign_in() : m.invite_to_register()}
              </Button>
            {/if}
          </form>
        {/if}
      {/await}
    </div>
  </main>
{/if}
