<!--
  The page the link mailed to an account's new address opens
  (`/confirm-email#<token>`). Opening it changes nothing, so that a mail
  scanner or a preview that opens the link does not change the address: the
  button does, whoever holds the link and whether or not this browser is
  signed in, and the page says what came of it. A link that changes nothing
  says so: asking again is done in the settings, with the account's password.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { problemOf } from './problems.ts';

  interface Props {
    /** The token of the link; empty for a link without one. */
    token: string;
    /** Changes the address by the token. */
    confirm: (token: string) => Promise<void>;
    homeHref: string;
  }

  let { token, confirm, homeHref }: Props = $props();

  /** The change, once the button asked for it. */
  let confirming = $state.raw<Promise<void>>();

  /** Whether the link itself changes nothing, and not the request that failed. */
  function linkRefused(error: unknown): boolean {
    return isBackendError(error) && error.errors.some((entry) => entry.pointer === '/token');
  }
</script>

<main class="grid min-h-dvh place-items-center p-4">
  <div class="grid w-full max-w-sm gap-4" data-testid="confirm-email">
    <div class="text-center">
      <p class="text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
      <h1 class="mt-2 text-lg font-medium">{m.confirm_email_title()}</h1>
    </div>
    {#snippet button()}
      <Button
        class="roomy:min-h-touch"
        data-testid="confirm-email-submit"
        onclick={() => (confirming = confirm(token))}>{m.confirm_email_submit()}</Button
      >
    {/snippet}
    {#if !token}
      <p role="alert">{m.confirm_email_invalid()}</p>
    {:else if !confirming}
      <p>{m.confirm_email_lead()}</p>
      {@render button()}
    {:else}
      {#await confirming}
        <p role="status">{m.confirm_email_working()}</p>
      {:then}
        <p role="status" data-testid="confirm-email-done">{m.confirm_email_done()}</p>
      {:catch error}
        {#if linkRefused(error)}
          <p role="alert">{m.confirm_email_invalid()}</p>
        {:else}
          <!-- The request failed, not the link: it may be asked again. -->
          <p role="alert">{problemOf(error)}</p>
          {@render button()}
        {/if}
      {/await}
    {/if}
    <a href={homeHref} class="text-center text-sm underline underline-offset-4"
      >{m.confirm_email_go()}</a
    >
  </div>
</main>
