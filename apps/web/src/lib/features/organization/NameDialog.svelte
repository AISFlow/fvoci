<!--
  Asks for the new name of a workspace. The field starts with the name as it
  is and holds the focus; Enter submits.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';

  interface Props {
    open: boolean;
    title: string;
    /** The name the field starts with. */
    initial: string;
    onSubmit: (name: string) => void;
  }

  let { open = $bindable(), title, initial, onSubmit }: Props = $props();

  let name = $state('');
  // Every opening starts from the name as it is now.
  $effect(() => {
    if (open) name = initial;
  });

  const trimmed = $derived(name.trim());

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    if (trimmed === '') return;
    open = false;
    onSubmit(trimmed);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content data-testid="name-dialog">
    <form class="grid gap-4" onsubmit={submit}>
      <Dialog.Header>
        <Dialog.Title>{title}</Dialog.Title>
      </Dialog.Header>
      <Input bind:value={name} aria-label={m.org_workspace_name()} maxlength={100} />
      <Dialog.Footer>
        <Button
          variant="outline"
          class="roomy:min-h-touch"
          onclick={() => {
            open = false;
          }}
        >
          {m.shared_cancel()}
        </Button>
        <Button type="submit" class="roomy:min-h-touch" disabled={trimmed === ''}>
          {m.org_rename()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
