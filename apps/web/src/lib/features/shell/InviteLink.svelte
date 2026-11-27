<!--
  The link of an invitation that was just made, to be copied and sent: the one
  time it is shown, as the server keeps no token it could show again.
-->
<script lang="ts">
  import InviteEmail from './InviteEmail.svelte';
  import type { InvitationMail } from '$lib/state/index.ts';
  import CopyIcon from '@lucide/svelte/icons/copy';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Input } from '$lib/ui/input/index.js';

  interface Props {
    /** The whole address of the link. */
    link: string;
    /** What the link is, for whoever cannot see the field. */
    label: string;
    /** What is said under the field: for whom the link is, and for how long. */
    note: string;
    testId: string;
    delivery?: {
      mail: InvitationMail;
      workspaceId: string;
      token: string;
      language: string;
      hint: string;
    };
  }

  let { link, label, note, testId, delivery }: Props = $props();

  let copied = $state(false);

  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(link);
      copied = true;
    } catch {
      // No clipboard (an insecure page, or denied): the field can still be selected.
      copied = false;
    }
  }
</script>

<div class="grid gap-1.5">
  <div class="flex items-center gap-2">
    <Input
      readonly
      value={link}
      aria-label={label}
      class="min-w-0 flex-1 roomy:h-touch"
      data-testid={testId}
      onfocus={(event: FocusEvent) => {
        (event.currentTarget as HTMLInputElement).select();
      }}
    />
    <IconButton
      label={copied ? m['ShareBoard.copiedLink']() : m['ShareBoard.copyLink']()}
      onclick={() => void copy()}
      data-testid="{testId}-copy"
    >
      <CopyIcon />
    </IconButton>
  </div>
  <p class="text-xs text-muted-foreground" aria-live="polite">
    {copied ? m['ShareBoard.copiedLink']() : note}
  </p>
</div>

{#if delivery}<InviteEmail {...delivery} />{/if}
