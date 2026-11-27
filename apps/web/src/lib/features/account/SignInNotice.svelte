<!--
  What the page shows when a sign-in through a provider came back refused
  to somebody who is signed in: the session they have is as it was (what asked
  for single sign-on stays closed, or nothing was connected), and
  the page says why, with the way on and the way to the connected sign-ins,
  where a provider is connected to the account.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  import { oidcProblemOf } from './oidc-problems.ts';

  interface Props {
    /** The name of what went wrong, from the address (`oidc_error`). */
    problem: string;
    onGoOn: () => void;
    onOpenLinked: () => void;
  }

  let { problem, onGoOn, onOpenLinked }: Props = $props();
</script>

<div class="grid h-full place-items-center p-6 text-center" data-testid="sign-in-notice">
  <div class="grid max-w-md justify-items-center gap-3">
    <p class="text-lg font-medium" role="alert">
      <!-- Signed in already, "sign in with its password" is not what there is to do. -->
      {problem === 'oidc-email-taken'
        ? m.account_oidc_email_taken_signed_in()
        : oidcProblemOf(problem)}
    </p>
    <div class="flex flex-wrap justify-center gap-2">
      <Button variant="outline" class="roomy:min-h-touch" onclick={onOpenLinked}>
        {m.account_linked_open()}
      </Button>
      <Button class="roomy:min-h-touch" onclick={onGoOn}>{m.account_notice_go_on()}</Button>
    </div>
  </div>
</div>
