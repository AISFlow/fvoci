<!--
  Signing in, and making an account, on a backend that has accounts (the
  server). The page the app shows while nobody is signed in. A new account
  starts on its own, in its own space, unless the reader chooses to work
  together and names what they start (`RegisterUse.svelte`); the server's
  first account names one.

  The two forms share the page: a link turns one into the other. Making an
  account does not sign in on the server, so the page signs in right after
  with what was typed; if that fails, the page is the sign-in form. A refusal
  is told as `problems.ts` says.

  Where the server takes nobody without an invitation (it has its organization), the
  page offers no form to make an account, and says what to ask for instead.

  Where the server confirms addresses, making an account makes nothing yet: the
  page says to check the mail, whose link makes the account and signs in. A
  sign-in to a registration that is not confirmed (its right password) is told
  so, with a button that mails the link again.

  An organization whose people sign in through its provider has a button
  for that above the form: the organization the page's address names
  (`/login?org=…`), or the only one of the server. A sign-in through the
  provider that went wrong comes back to this page with the name of what
  went wrong in `oidc_error`, which is told as a refusal is.

  While that organization moves to another provider, a sign-in of somebody the
  new provider does not know yet comes back with `oidc_held`: the page then asks
  whether they had an account before it shows any form (`SignInChoice.svelte`).
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import {
    isBackendError,
    type LoginOptions,
    type Registered,
    type Registration,
  } from '$lib/state/index.ts';
  import type { WaitingAccount } from '$lib/state/email-verification.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import { oidcProblemOf } from './oidc-problems.ts';
  import AccountFields from './AccountFields.svelte';
  import CheckMailPage from './CheckMailPage.svelte';
  import RegisterUse from './RegisterUse.svelte';
  import ResendLink from './ResendLink.svelte';
  import SignInChoice from './SignInChoice.svelte';
  import {
    accountFieldsUnder,
    formProblemsOf,
    problemOf,
    type AccountField,
    type FieldProblems,
  } from './problems.ts';

  interface Props {
    onSignIn: (login: string, password: string) => Promise<void>;
    /** Makes the account, or nothing yet where its address is to be confirmed (`waiting`). */
    onRegister: (registration: Registration) => Promise<Registered>;
    /** Mails a registration that waits for its address a new link (`EmailVerification.resend`). */
    onResend?: (waiting: WaitingAccount) => Promise<void>;
    /** What the page shows of the server (`Account.loginOptions`). */
    optionsOf?: (orgId: string | undefined) => Promise<LoginOptions>;
    /**
     * Starts a sign-in through the provider and sends the browser to it
     * (`Account.startOidc`); a refusal of the start rejects.
     */
    onOidcStart?: (orgId: string) => Promise<void>;
    /**
     * For a sign-in held for this browser: sends the browser to the provider the
     * organization used before (`Account.startOidcCarryOver`); a refusal rejects.
     */
    onCarryOver?: (orgId: string) => Promise<void>;
    /** Ends the held sign-in in a new account, signed in (`Account.finishHeldSignIn`). */
    onStartAnew?: (orgId: string) => Promise<void>;
    /** Where "Forgot your password?" leads; the link shows only while the server sends mail. */
    forgotHref?: string;
    /**
     * The page was started by asking for the account's deletion: the days the
     * account waits, said above the form, for the sign-out is not unexplained.
     */
    leavingDays?: number;
  }

  let {
    onSignIn,
    onRegister,
    onResend,
    optionsOf,
    onOidcStart,
    onCarryOver,
    onStartAnew,
    forgotHref,
    leavingDays,
  }: Props = $props();

  /** What the page's address says: the organization, and what went wrong. */
  const address = new URLSearchParams(location.search);
  // An empty `org` names none: the server's only organization, if it has one.
  const askedOrg = address.get('org') || undefined;

  /** What the server says of itself, once read; undefined if it could not be. */
  const options = $derived(optionsOf?.(askedOrg).catch(() => undefined));
  /** The organization with a provider, once read; undefined while there is none. */
  const organization = $derived(
    options?.then((read) => (read?.organization?.oidc ? read.organization : undefined)),
  );

  const oidcError = address.get('oidc_error');

  /** The organization a sign-in is held for, until the reader goes back to the form. */
  let heldFor = $state(address.has('oidc_held') ? askedOrg : undefined);

  /** The name of the organization a sign-in is held for, once the server has said it. */
  const heldName = $derived(
    options?.then((read) => read?.organization?.name ?? '') ?? Promise.resolve(''),
  );

  /** Back to the form: the address names the organization, and no held sign-in. */
  function leaveChoice(): void {
    if (heldFor === undefined) return;
    const named = new URLSearchParams({ org: heldFor });
    history.replaceState(history.state, '', `${location.pathname}?${named}`);
    heldFor = undefined;
  }

  let mode = $state<'signIn' | 'register'>('signIn');
  let login = $state('');
  let organizationName = $state('');
  /** The new account starts an organization, named `organizationName`; on its own otherwise. */
  let together = $state(false);
  let username = $state('');
  let displayName = $state('');
  let email = $state('');
  let password = $state('');
  let working = $state(false);
  let problem = $state<string | undefined>(oidcError ? oidcProblemOf(oidcError) : undefined);

  /** The address a registration's link went to, while the page waits for it. */
  let waitingFor = $state<string | undefined>(undefined);
  /** The name or address a sign-in was refused for, as not confirmed yet. */
  let unconfirmed = $state<string | undefined>(undefined);

  /** What is wrong with each field, where the server said so of the field. */
  let fieldProblems = $state.raw<FieldProblems>({});

  /**
   * The field of each form that a member of its request is typed into, by the
   * member's JSON Pointer. Signing in sends what was typed as the name and as
   * the email address.
   */
  const FIELDS: Record<'signIn' | 'register', Record<string, AccountField>> = {
    signIn: { '/username': 'login', '/email': 'login', '/password': 'password' },
    register: { ...accountFieldsUnder(''), '/organizationName': 'organizationName' },
  };

  /** Starts a sign-in through the organization's provider; says why it did not. */
  async function startOidc(orgId: string): Promise<void> {
    if (working || !onOidcStart) return;
    working = true;
    problem = undefined;
    try {
      // Started, the browser is leaving for the provider: the page stays busy,
      // so that a second press starts no second sign-in in place of this one.
      await onOidcStart(orgId);
    } catch (error) {
      problem =
        isBackendError(error) && error.reason === 'oidc_unavailable'
          ? m.account_oidc_failed()
          : problemOf(error);
      working = false;
    }
  }

  /** Back from the provider by the browser's back button, the page as it was left: no longer busy. */
  function onpageshow(event: PageTransitionEvent): void {
    if (event.persisted) working = false;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    working = true;
    problem = undefined;
    unconfirmed = undefined;
    fieldProblems = {};
    try {
      if (mode === 'register') {
        const required = (await options)?.organizationRequired === true;
        const named = together || required;
        const started = organizationName.trim();
        // Blank, the server makes an account on its own: refused here when one chose to start
        // an organization. The server's first account it refuses itself, with all else wrong.
        if (together && !required && started === '') {
          fieldProblems = { organizationName: m.account_field_required() };
          await tick();
          document.getElementById('account-organizationName')?.focus();
          return;
        }
        const registered = await onRegister({
          ...(named ? { organizationName: started } : {}),
          username: username.trim(),
          // Trimmed by the server, as it counts white space.
          displayName,
          email: email.trim(),
          password,
        });
        if (registered === 'waiting') {
          password = '';
          waitingFor = email.trim();
          return;
        }
        // The account is there now. Should signing in fail, the form to try
        // again is the one to sign in with, the name filled in.
        login = username.trim();
        mode = 'signIn';
        await onSignIn(login, password);
      } else {
        await onSignIn(login.trim(), password);
      }
    } catch (error) {
      if (mode === 'signIn' && isBackendError(error) && error.reason === 'email_unverified') {
        unconfirmed = login.trim();
      }
      // `mode` is the form the refused request was sent from: the link that
      // turns one form into the other is off while a request is under way.
      const said = formProblemsOf(error, FIELDS[mode]);
      fieldProblems = said.fields;
      problem = said.problem;
      // The first field that is wrong is where to go on.
      if (said.first !== undefined) {
        await tick();
        document.getElementById(`account-${said.first}`)?.focus();
      }
    } finally {
      working = false;
    }
  }

  function switchTo(next: 'signIn' | 'register'): void {
    mode = next;
    problem = undefined;
    unconfirmed = undefined;
    fieldProblems = {};
  }

  /** Back from waiting to the form, to register with another address: its first field. */
  async function signUpAgain(): Promise<void> {
    waitingFor = undefined;
    switchTo('register');
    await tick();
    (
      document.getElementById('account-organizationName') ??
      document.getElementById('account-username')
    )?.focus();
  }
</script>

<svelte:window {onpageshow} />

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

{#if waitingFor !== undefined}
  {@const address = waitingFor}
  <CheckMailPage
    email={address}
    onResend={() => onResend?.({ email: address }) ?? Promise.resolve()}
    onSignUpAgain={() => void signUpAgain()}
  />
{:else if heldFor !== undefined && onCarryOver && onStartAnew}
  {@const orgId = heldFor}
  <SignInChoice
    name={heldName}
    onCarryOver={() => onCarryOver(orgId)}
    onStartAnew={() => onStartAnew(orgId)}
    onCancel={leaveChoice}
  />
{:else}
  <main class="grid min-h-dvh place-items-center p-4">
    <form
      class="grid w-full max-w-sm gap-4"
      onsubmit={(event) => void submit(event)}
      data-testid={mode === 'signIn' ? 'sign-in-form' : 'register-form'}
    >
      <div class="text-center">
        <p class="text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
        <h1 class="mt-2 text-lg font-medium">
          {mode === 'signIn' ? m.account_sign_in_title() : m.account_register_title()}
        </h1>
      </div>

      {#if mode === 'signIn' && onOidcStart}
        {#await organization then found}
          {#if found}
            <Button
              type="button"
              onclick={() => void startOidc(found.id)}
              disabled={working}
              variant="outline"
              class="roomy:min-h-touch"
              data-testid="oidc-continue"
            >
              {found.name
                ? m.account_oidc_continue({ name: found.name })
                : m.account_oidc_continue_unnamed()}
            </Button>
            <p class="text-center text-sm text-muted-foreground">{m.account_oidc_or()}</p>
          {/if}
        {/await}
      {/if}

      {#if mode === 'signIn'}
        <div class="grid gap-1.5">
          <Label for="account-login">{m.account_login()}</Label>
          <Input
            id="account-login"
            name="login"
            bind:value={login}
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
            autocomplete="current-password"
            required
            aria-invalid={fieldProblems.password ? 'true' : undefined}
            aria-describedby={fieldProblems.password ? 'account-password-problem' : undefined}
            class="roomy:h-touch roomy:text-base"
          />
          {@render fieldProblem('password')}
        </div>
      {:else}
        {#await options then read}
          <RegisterUse
            bind:together
            bind:name={organizationName}
            required={read?.organizationRequired === true}
            problem={fieldProblems.organizationName}
          />
        {/await}
        <AccountFields
          bind:username
          bind:displayName
          bind:email
          bind:password
          problems={fieldProblems}
        />
        {#await options then read}
          {#if read?.emailVerification}
            <p class="text-xs text-muted-foreground" data-testid="verify-hint">
              {m.verify_register_hint()}
            </p>
          {/if}
        {/await}
      {/if}

      {#if problem}
        <p class="text-sm text-destructive" role="alert">{problem}</p>
      {:else if leavingDays !== undefined && mode === 'signIn'}
        <p class="text-sm text-muted-foreground" role="status" data-testid="account-leaving-asked">
          {m.account_delete_asked({ days: leavingDays })}
        </p>
      {/if}
      {#if unconfirmed !== undefined && onResend}
        {@const named = unconfirmed}
        <!-- The name or the address: the server looks for a registration by either. -->
        <ResendLink onResend={() => onResend({ username: named, email: named })} />
      {/if}

      <Button type="submit" class="roomy:min-h-touch" disabled={working}>
        {#if working}
          {m.account_working()}
        {:else}
          {mode === 'signIn' ? m.account_sign_in() : m.account_register()}
        {/if}
      </Button>

      {#if mode === 'signIn' && forgotHref}
        {#await options then read}
          {#if read?.mailEnabled}
            <a href={forgotHref} class="text-center text-sm underline underline-offset-4">
              {m.reset_forgot()}
            </a>
          {/if}
        {/await}
      {/if}

      <!-- Until the server has said whether it takes an account, neither is offered. -->
      {#await options then read}
        {#if mode === 'register' || read?.registrationOpen !== false}
          <Button
            variant="link"
            class="roomy:min-h-touch"
            disabled={working}
            onclick={() => {
              switchTo(mode === 'signIn' ? 'register' : 'signIn');
            }}
          >
            {mode === 'signIn' ? m.account_to_register() : m.account_to_sign_in()}
          </Button>
        {:else}
          <p class="text-center text-sm text-muted-foreground" data-testid="invitation-needed">
            {m.account_invitation_needed()}
          </p>
        {/if}
      {/await}
    </form>
  </main>
{/if}
