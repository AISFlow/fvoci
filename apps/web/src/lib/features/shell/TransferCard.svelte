<!--
  One transfer of the stack (Transfers.svelte): what it is doing and how far it
  is, what it did, or why an upload failed and, where it can be, a way to send
  it again. Its button takes an ended one away, and gives one under way up
  once the reader says so (`asking`): a small button at a corner is easily hit.
-->
<script lang="ts">
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Transfer } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import TransferAsk from './TransferAsk.svelte';
  import { failureText, progressText, transferLabel, underWay } from './transfer-words.ts';

  interface Props {
    transfer: Transfer;
    /** The reader is asked whether to give it up. */
    asking: boolean;
    /** Its button was pressed: one under way is asked about first, an ended one goes. */
    onClose: () => void;
    /** The answer: give it up, or keep it going. */
    onAnswer: (stop: boolean) => void;
    /** A row of a group's list rather than a card of its own. */
    row?: boolean;
  }

  let { transfer, asking, onClose, onAnswer, row = false }: Props = $props();

  const going = $derived(underWay(transfer));
  const name = $derived(transfer.name || m['Sidebar.untitled-board']());
  const label = $derived(transferLabel(transfer));
  const why = $derived(failureText(transfer));
  const progress = $derived(progressText(transfer));
</script>

<li
  class={[
    'grid gap-1.5 text-sm',
    row
      ? 'border-t py-2 first:border-t-0'
      : 'pointer-events-auto rounded-md border bg-popover p-3 text-popover-foreground shadow-md max-sm:p-2',
  ]}
  data-transfer={transfer.kind}
  data-id={transfer.id}
  data-finished={transfer.finished ? '' : undefined}
  data-failed={transfer.failure}
  data-name={transfer.name}
>
  <div class="flex items-start gap-2">
    {#if transfer.failure}
      <CircleAlertIcon class="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
    {:else if transfer.finished}
      <CircleCheckIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
    {/if}
    <p class="min-w-0 flex-1 [overflow-wrap:anywhere]" title={label}>{label}</p>
    <IconButton
      label={going ? m.shell_transfer_stop({ name }) : m.shell_transfer_dismiss({ name })}
      class="-my-1 -mr-1 size-6"
      data-close
      onclick={onClose}
    >
      <XIcon />
    </IconButton>
  </div>
  {#if why}
    <p class="text-xs text-destructive">{why}</p>
  {/if}
  {#if transfer.retry}
    <div>
      <Button
        size="sm"
        variant="outline"
        class="roomy:min-h-touch"
        aria-label={m.shell_transfer_retry_name({ name })}
        onclick={transfer.retry}
      >
        {m.shell_transfer_retry()}
      </Button>
    </div>
  {/if}
  {#if going && asking}
    <TransferAsk {onAnswer} />
  {:else if going}
    <progress
      class="h-1.5 w-full accent-primary"
      max={transfer.total || undefined}
      value={transfer.total ? transfer.done : undefined}
      aria-label={label}
    ></progress>
    {#if progress}
      <p class={['text-xs text-muted-foreground', !row && 'max-sm:hidden']}>{progress}</p>
    {/if}
  {/if}
</li>
