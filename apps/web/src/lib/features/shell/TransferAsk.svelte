<!--
  Whether to give up what is under way (TransferCard, a group of uploads in
  Transfers.svelte): asked in place, the answer that loses nothing focused.
  Escape keeps it going.
-->
<script lang="ts">
  import type { Attachment } from 'svelte/attachments';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  let { onAnswer }: { onAnswer: (stop: boolean) => void } = $props();

  const focused: Attachment<HTMLElement> = (element) => {
    element.focus();
  };

  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== 'Escape') return;
    event.preventDefault();
    event.stopPropagation();
    onAnswer(false);
  }
</script>

<div class="grid gap-1.5" role="group" aria-label={m.shell_transfer_ask()}>
  <p class="text-xs">{m.shell_transfer_ask()}</p>
  <div class="flex flex-wrap gap-2">
    <Button
      size="sm"
      variant="destructive"
      class="roomy:min-h-touch"
      data-stop
      {onkeydown}
      onclick={() => {
        onAnswer(true);
      }}
    >
      {m.shell_transfer_ask_stop()}
    </Button>
    <Button
      size="sm"
      variant="outline"
      class="roomy:min-h-touch"
      {@attach focused}
      {onkeydown}
      onclick={() => {
        onAnswer(false);
      }}
    >
      {m.shell_transfer_ask_keep()}
    </Button>
  </div>
</div>
