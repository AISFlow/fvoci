<!--
  After a registration where the server confirms addresses: nothing is made
  yet, and the link mailed to the address makes the account and signs in. The
  page says where the link went, mails it again, and leads back to the form
  to register again with the address put right: from this browser the new
  registration replaces the one that waits, under the same user name.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  import ResendLink from './ResendLink.svelte';

  interface Props {
    /** The address the link went to. */
    email: string;
    /** Mails the registration a new link. */
    onResend: () => Promise<void>;
    /** Back to the form, to register again with the address put right. */
    onSignUpAgain?: () => void;
  }

  let { email, onResend, onSignUpAgain }: Props = $props();

  /** Takes the reader to what replaced the form they were on. */
  function focused(heading: HTMLElement): void {
    heading.focus();
  }
</script>

<main class="grid min-h-dvh place-items-center p-4">
  <section class="grid w-full max-w-sm gap-4" data-testid="check-mail">
    <div class="text-center">
      <p class="text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
      <h1 class="mt-2 text-lg font-medium" tabindex="-1" {@attach focused}>
        {m.verify_check_title()}
      </h1>
    </div>
    <p class="break-words">{m.verify_check_sent({ email })}</p>
    <p class="text-sm text-muted-foreground">{m.verify_check_until()}</p>
    <ResendLink {onResend} />
    {#if onSignUpAgain}
      <p class="text-sm text-muted-foreground">{m.verify_wrong_address()}</p>
      <Button
        type="button"
        variant="link"
        class="roomy:min-h-touch"
        onclick={onSignUpAgain}
        data-testid="sign-up-again"
      >
        {m.verify_sign_up_again()}
      </Button>
    {/if}
  </section>
</main>
