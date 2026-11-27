<!--
  A button that mails a registration a new confirmation link, and what came
  of it: the same words whether or not a registration waits, as the server
  answers the same.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  import { problemOf } from './problems.ts';

  interface Props {
    /** Mails the new link. */
    onResend: () => Promise<void>;
  }

  let { onResend }: Props = $props();

  let working = $state(false);
  let resent = $state(false);
  let problem = $state<string | undefined>(undefined);

  async function resend(): Promise<void> {
    if (working) return;
    working = true;
    resent = false;
    problem = undefined;
    try {
      await onResend();
      resent = true;
    } catch (error) {
      problem = problemOf(error);
    } finally {
      working = false;
    }
  }
</script>

<div class="grid gap-2">
  <Button
    type="button"
    variant="outline"
    class="roomy:min-h-touch"
    disabled={working}
    onclick={() => void resend()}
    data-testid="resend-link"
  >
    {m.verify_resend()}
  </Button>
  <!-- Present from the start, so that what it comes to say is announced. -->
  <p class="text-sm" role="status" data-testid="resend-status">
    {resent ? m.verify_resent() : ''}
  </p>
  {#if problem}
    <p class="text-sm text-destructive" role="alert">{problem}</p>
  {/if}
</div>
