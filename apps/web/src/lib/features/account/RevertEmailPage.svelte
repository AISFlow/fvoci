<!--
  The page the link mailed to an account's address before a change opens
  (`/revert-email#<token>`): for its owner, should somebody else have changed
  it. Opening it changes nothing; the button gives the address back, whoever
  holds the link and whether or not this browser is signed in, and every
  session of the account ends. The page then says so, and leads to a reset
  link for the restored address (or asks to contact an administrator when the
  previous address was already taken and the account's email was cleared): the
  password works no more until a new one is set. A link that changes nothing
  says only that.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { problemOf } from './problems.ts';

  interface Props {
    /** The token of the link; empty for a link without one. */
    token: string;
    /** Gives the address back by the token: the address, or none where the account's address was cleared. */
    revert: (token: string) => Promise<string | undefined>;
    homeHref: string;
    /** The page that asks for a reset link, filled in with the restored `email`. */
    resetHref: (email: string) => string;
  }

  let { token, revert, homeHref, resetHref }: Props = $props();

  /** The use of the link, once the button asked for it. */
  let reverting = $state.raw<Promise<string | undefined>>();

  /** Whether the link itself changes nothing, and not the request that failed. */
  function linkRefused(error: unknown): boolean {
    return isBackendError(error) && error.errors.some((entry) => entry.pointer === '/token');
  }
</script>

<main class="grid min-h-dvh place-items-center p-4">
  <div class="grid w-full max-w-sm gap-4" data-testid="revert-email">
    <div class="text-center">
      <p class="text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
      <h1 class="mt-2 text-lg font-medium">{m.revert_email_title()}</h1>
    </div>
    {#snippet button()}
      <Button
        class="roomy:min-h-touch"
        data-testid="revert-email-submit"
        onclick={() => (reverting = revert(token))}
      >
        {m.revert_email_submit()}
      </Button>
    {/snippet}
    {#if !token}
      <p role="alert">{m.revert_email_invalid()}</p>
    {:else if !reverting}
      <p>{m.revert_email_lead()}</p>
      {@render button()}
    {:else}
      {#await reverting}
        <p role="status">{m.revert_email_working()}</p>
      {:then email}
        <div class="grid gap-2" role="status" data-testid="revert-email-done">
          {#if email}
            <p>{m.revert_email_done({ email })}</p>
            <p>{m.revert_email_password()}</p>
          {:else}
            <p>{m.revert_email_cleared()}</p>
            <p>{m.revert_email_cleared_password()}</p>
          {/if}
        </div>
        {#if email}
          <Button
            href={resetHref(email)}
            class="roomy:min-h-touch"
            data-testid="revert-email-reset"
          >
            {m.revert_email_reset()}
          </Button>
        {/if}
      {:catch error}
        {#if linkRefused(error)}
          <p role="alert">{m.revert_email_invalid()}</p>
        {:else}
          <!-- The request failed, not the link: it may be asked again. -->
          <p role="alert">{problemOf(error)}</p>
          {@render button()}
        {/if}
      {/await}
    {/if}
    <a href={homeHref} class="text-center text-sm underline underline-offset-4">
      {m.reset_sign_in()}
    </a>
  </div>
</main>
