<!--
  The page the confirmation link opens (`/verify-email#<token>`): in the
  browser the registration was made from it confirms at once, which signs the
  browser in, and goes to where the account was let in. Anywhere else the
  server asks for the password the registration was made with, and the page
  for it. A link that confirms nothing says why, and offers a new one for the
  address typed in.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError } from '$lib/state/index.ts';
  import type { Confirmed } from '$lib/state/email-verification.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import { problemOf } from './problems.ts';

  interface Props {
    /**
     * The confirmation by the link's token, sent once by whoever opened the
     * page, which goes on to where the account was let in; `undefined` for a
     * link without a token.
     */
    confirming: Promise<Confirmed> | undefined;
    homeHref: string;
    /** Mails a new link for the registration of the address. */
    resend: (email: string) => Promise<void>;
    /**
     * Confirms again with the password the registration was made with: what
     * `confirming` becomes is that attempt.
     */
    onPassword: (password: string) => void;
  }

  let { confirming, homeHref, resend, onPassword }: Props = $props();

  /** The refusal of a link without a token, which confirms nothing. */
  const NO_TOKEN = new Error('the link has no token');
  /** What the page waits for. */
  const shown = $derived(confirming ?? Promise.reject(NO_TOKEN));

  let email = $state('');
  let password = $state('');
  /** Whether a password was sent: what came of it is told beside its field. */
  let passwordSent = $state(false);
  let working = $state(false);
  let resent = $state(false);
  let problem = $state<string | undefined>(undefined);

  /**
   * Whether the link itself confirms nothing (none, ended, used, replaced),
   * and not the request that failed: a new link would end one that still works.
   */
  function linkRefused(error: unknown): boolean {
    return (
      error === NO_TOKEN ||
      (isBackendError(error) && error.errors.some((entry) => entry.pointer === '/token'))
    );
  }

  /**
   * Whether the link would confirm, and the server wants the password of the
   * registration with it: asked for the first time, or again after one that
   * was wrong, one too many, or got no answer. Any other refusal of a password
   * that was sent is of the registration itself, which is gone by then.
   */
  function passwordWanted(error: unknown): boolean {
    if (!isBackendError(error)) return passwordSent && error !== NO_TOKEN;
    if (error.reason === 'password_required') return true;
    return (
      passwordSent &&
      (error.reason === 'invalid_credentials' || error.status === 429 || error.status === 0)
    );
  }

  /** What is wrong with the password that was sent; nothing before one was. */
  function passwordProblem(error: unknown): string | undefined {
    if (isBackendError(error) && error.reason === 'password_required') return undefined;
    if (isBackendError(error) && error.reason === 'invalid_credentials') {
      return m.verify_password_wrong();
    }
    return problemOf(error);
  }

  function sendPassword(event: SubmitEvent): void {
    event.preventDefault();
    passwordSent = true;
    onPassword(password);
  }

  /** Takes the reader to the field the page asks for. */
  function focused(field: HTMLElement): void {
    field.focus();
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    working = true;
    resent = false;
    problem = undefined;
    try {
      await resend(email.trim());
      resent = true;
    } catch (error) {
      problem = problemOf(error);
    } finally {
      working = false;
    }
  }
</script>

<main class="grid min-h-dvh place-items-center p-4">
  <div class="grid w-full max-w-sm gap-4">
    <div class="text-center">
      <p class="text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
      <h1 class="mt-2 text-lg font-medium">{m.verify_title()}</h1>
    </div>
    {#await shown}
      <p role="status" data-testid="verify-status">{m.verify_working()}</p>
    {:then}
      <p role="status" data-testid="verify-status">{m.verify_done()}</p>
    {:catch error}
      {#if linkRefused(error)}
        <p role="alert" data-testid="verify-problem">{m.verify_invalid()}</p>
        <form
          class="grid gap-4"
          onsubmit={(event) => void submit(event)}
          data-testid="verify-request-form"
        >
          <div class="grid gap-1.5">
            <Label for="verify-email">{m.account_email()}</Label>
            <Input
              id="verify-email"
              type="email"
              autocomplete="email"
              bind:value={email}
              required
              disabled={working}
              class="roomy:h-touch roomy:text-base"
            />
          </div>
          <Button type="submit" disabled={working} class="roomy:min-h-touch">
            {m.verify_request_submit()}
          </Button>
          <p class="text-sm" role="status">{resent ? m.verify_resent() : ''}</p>
          {#if problem}
            <p class="text-sm text-destructive" role="alert">{problem}</p>
          {/if}
        </form>
      {:else if passwordWanted(error)}
        {@const wrong = passwordProblem(error)}
        <p data-testid="verify-password-intro">{m.verify_password_intro()}</p>
        <form class="grid gap-4" onsubmit={sendPassword} data-testid="verify-password-form">
          <div class="grid gap-1.5">
            <Label for="verify-password">{m.account_password()}</Label>
            <Input
              id="verify-password"
              type="password"
              autocomplete="current-password"
              bind:value={password}
              required
              aria-invalid={wrong ? 'true' : undefined}
              aria-describedby={wrong ? 'verify-password-problem' : undefined}
              class="roomy:h-touch roomy:text-base"
              {@attach focused}
            />
            {#if wrong}
              <p id="verify-password-problem" class="text-sm text-destructive" role="alert">
                {wrong}
              </p>
            {/if}
          </div>
          <Button type="submit" class="roomy:min-h-touch">{m.verify_password_submit()}</Button>
        </form>
      {:else}
        <p role="alert" data-testid="verify-problem">{problemOf(error)}</p>
      {/if}
    {/await}
    <a href={homeHref} class="text-center text-sm underline underline-offset-4"
      >{m.reset_sign_in()}</a
    >
  </div>
</main>
