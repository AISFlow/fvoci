<!--
  The public read-only link section inside `ShareDialog.svelte`: toggling the
  link on/off, copying it, and regenerating its token after confirmation.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import CheckIcon from '@lucide/svelte/icons/check';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError, type PublicLink, type Workspace } from '$lib/state/index.ts';
  import { Input } from '$lib/ui/input/index.js';
  import { Switch } from '$lib/ui/switch/index.js';

  interface Props {
    workspace: Workspace;
    containerId: string;
    linkOf: (readToken: string) => string;
    linkOnly: boolean;
    /** Incremented by the dialog each time it opens to re-read sharing state. */
    openVersion: number;
  }

  let { workspace, containerId, linkOf, linkOnly, openVersion }: Props = $props();

  const uid = $props.id();

  let publicLink = $state.raw<PublicLink | undefined>(undefined);
  /** The server does not allow public links (403). */
  let sharingOff = $state(false);
  let sharingBusy = $state(false);
  let copied = $state(false);
  /** Made anew when a change of the link is refused: the switch shows the link as it is again. */
  let switchVersion = $state(0);
  /** Asking before a new link stops the old one. */
  let confirmOpen = $state(false);
  let readings = 0;

  const link = $derived(publicLink?.enabled && publicLink.token ? linkOf(publicLink.token) : '');

  async function readPublicLink(id: string): Promise<void> {
    readings += 1;
    const reading = readings;
    publicLink = undefined;
    sharingOff = false;
    copied = false;
    try {
      const read = await workspace.backend.publicLink(id);
      if (reading === readings && id === containerId) publicLink = read;
    } catch (error) {
      if (reading === readings && id === containerId) workspace.report('load', error);
    }
  }

  function syncPublicLink(): () => void {
    const id = containerId;
    const version = openVersion;
    untrack(() => {
      if (version >= 0) void readPublicLink(id);
    });
    return () => {
      readings += 1;
    };
  }

  async function changePublicLink(enabled: boolean, regenerate = false): Promise<void> {
    if (sharingBusy) return;
    const id = containerId;
    readings += 1;
    const reading = readings;
    sharingBusy = true;
    copied = false;
    try {
      const changed = await workspace.backend.setPublicLink(id, { enabled, regenerate });
      if (reading === readings && id === containerId) publicLink = changed;
    } catch (error) {
      if (reading !== readings || id !== containerId) return;
      if (isBackendError(error) && error.reason === 'sharing_disabled') sharingOff = true;
      else workspace.report('save', error);
      switchVersion += 1;
    } finally {
      sharingBusy = false;
    }
  }

  async function copyLink(): Promise<void> {
    try {
      await navigator.clipboard.writeText(link);
      copied = true;
    } catch {
      // No clipboard (an insecure page, or denied): the field can still be selected.
      copied = false;
    }
  }
</script>

<section
  class={['grid gap-2', !linkOnly && 'border-t pt-4']}
  aria-labelledby="{uid}-public"
  data-testid="share-public-section"
  {@attach syncPublicLink}
>
  <h3 id="{uid}-public" class="font-medium">{m.share_public_link()}</h3>
  <label class="flex min-h-8 items-center gap-3 roomy:min-h-touch">
    <span class="min-w-0 flex-1 text-muted-foreground">
      {m['ShareBoard.PublishDescription']()}
    </span>
    {#key switchVersion}
      <Switch
        checked={publicLink?.enabled ?? false}
        disabled={!publicLink || sharingBusy || sharingOff}
        data-testid="share-public"
        onCheckedChange={(checked: boolean) => void changePublicLink(checked)}
      />
    {/key}
  </label>
  {#if sharingOff}
    <p class="text-muted-foreground" role="status">{m.share_public_off()}</p>
  {/if}
  {#if link}
    <div class="flex items-center gap-1">
      <Input
        readonly
        value={link}
        aria-label={m.share_public_link()}
        data-testid="share-link"
        class="min-w-0 flex-1 roomy:h-touch"
        onfocus={(event: FocusEvent) => {
          (event.currentTarget as HTMLInputElement).select();
        }}
      />
      <IconButton
        label={copied ? m['ShareBoard.copiedLink']() : m['ShareBoard.copyLink']()}
        onclick={() => void copyLink()}
      >
        {#if copied}<CheckIcon />{:else}<CopyIcon />{/if}
      </IconButton>
      <IconButton
        label={m['ShareBoard.regenerate']()}
        disabled={sharingBusy}
        data-testid="share-regenerate"
        onclick={() => {
          confirmOpen = true;
        }}
      >
        <RefreshCwIcon />
      </IconButton>
    </div>
  {/if}
</section>

<ConfirmDialog
  bind:open={confirmOpen}
  title={m['ShareBoard.regenerate']()}
  description={m['ShareBoard.confirmRegenerateToken']()}
  confirmLabel={m['ShareBoard.regenerate']()}
  onConfirm={() => void changePublicLink(true, true)}
/>
