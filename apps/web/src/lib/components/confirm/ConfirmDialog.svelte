<!--
  Asks before something is done that the reader may not want: deleting a
  board, a property with its values, a view. The question and the word on
  the button are the caller's; "Cancel" is always the other choice and is
  where the focus starts.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import * as AlertDialog from '$lib/ui/alert-dialog/index.js';

  interface Props {
    open: boolean;
    title: string;
    /** What will happen, in a sentence. */
    description?: string;
    /** The word on the button that goes ahead, such as "Delete". */
    confirmLabel: string;
    /** Draws the button that goes ahead in the colour of danger. */
    destructive?: boolean;
    onConfirm: () => void;
    /**
     * Puts the focus somewhere once the dialog has closed, in place of where it
     * was before (which the confirmed action may have taken away).
     */
    focusAfter?: (() => HTMLElement | null | undefined) | undefined;
  }

  let {
    open = $bindable(),
    title,
    description,
    confirmLabel,
    destructive = false,
    onConfirm,
    focusAfter,
  }: Props = $props();
</script>

<AlertDialog.Root bind:open>
  <AlertDialog.Content
    data-testid="confirm-dialog"
    onCloseAutoFocus={(event: Event) => {
      const target = focusAfter?.();
      if (!target) return;
      event.preventDefault();
      target.focus();
    }}
  >
    <AlertDialog.Header>
      <AlertDialog.Title>{title}</AlertDialog.Title>
      {#if description}
        <AlertDialog.Description>{description}</AlertDialog.Description>
      {/if}
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel class="roomy:min-h-touch">{m.shared_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant={destructive ? 'destructive' : 'default'}
        class="roomy:min-h-touch"
        onclick={() => {
          open = false;
          onConfirm();
        }}
      >
        {confirmLabel}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
