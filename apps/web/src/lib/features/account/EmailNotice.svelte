<!--
  One line for the reader's own space: mail that resets the password goes to
  the account's address, so an address the reader may lose (a company's) is
  better changed to their own, in the settings. An account without an address
  (one made through single sign-on) is told that there is nowhere for that
  mail to go, and to add one. It stands at the foot of the pane, takes no
  focus, and is closed for good (`onDismiss`).
-->
<script lang="ts">
  import MailIcon from '@lucide/svelte/icons/mail';
  import XIcon from '@lucide/svelte/icons/x';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    /** Whether the account has an address. */
    hasAddress: boolean;
    onDismiss: () => void;
  }

  let { hasAddress, onDismiss }: Props = $props();
</script>

<section
  class="flex w-full max-w-md items-start gap-3 rounded-lg border bg-popover p-3 text-popover-foreground shadow-lg"
  aria-label={m.email_change_title()}
  data-testid="email-notice"
>
  <MailIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  <p class="min-w-0 flex-1 text-sm">
    {hasAddress ? m.email_notice_text() : m.email_notice_none()}
  </p>
  <Button
    variant="ghost"
    size="icon"
    class="shrink-0 roomy:size-touch"
    aria-label={m.password_notice_dismiss()}
    title={m.password_notice_dismiss()}
    data-testid="email-notice-dismiss"
    onclick={onDismiss}
  >
    <XIcon aria-hidden="true" />
  </Button>
</section>
