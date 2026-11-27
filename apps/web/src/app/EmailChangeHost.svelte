<!--
  The address of the reader's account in the settings
  (`features/account/EmailChange`): asked to change through the account of the
  backend, and read anew after each asking, and as the section opens, since the
  link mailed to the new address is mostly opened in another browser. A
  session that is over is told as everywhere (`Workspace.report`). Whether the
  server mails is read with it; until it is, the section speaks as where it does.
-->
<script lang="ts">
  import { onMount } from 'svelte';

  import EmailChange from '$lib/features/account/EmailChange.svelte';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import { isSignedOut, rereadEmail, type Account, type Workspace } from '$lib/state/index.ts';

  let { workspace, account }: { workspace: Workspace; account: Account } = $props();

  let mails = $state(true);

  onMount(() => {
    void rereadEmail(workspace);
    void account
      .loginOptions(undefined)
      .then((options) => {
        mails = options.mailEnabled === true;
      })
      .catch(() => undefined);
  });

  /** Does it, and reads what became of the address whether or not it was refused. */
  async function then(doing: Promise<void>): Promise<void> {
    try {
      await doing;
    } catch (error) {
      if (isSignedOut(error)) workspace.report('save', error);
      throw error;
    } finally {
      await rereadEmail(workspace);
    }
  }
</script>

{#if workspace.me}
  <EmailChange
    email={workspace.me.email}
    pending={workspace.me.pendingEmail}
    hasPassword={workspace.me.hasPassword !== false}
    {mails}
    onRequest={(email: string, password: string) =>
      then(account.emailChange.request(email, password, getLocale()))}
    onResend={() => then(account.emailChange.resend(getLocale()))}
    onCancel={() => then(account.emailChange.cancel())}
  />
{/if}
