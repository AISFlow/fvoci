<!--
  Asks for a diary's new name. The field starts with the name as it is and
  holds the focus; Enter submits. An empty name is a diary without one, shown
  by the word for a diary.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';

  interface Props {
    open: boolean;
    /** The name the field starts with. */
    initial: string;
    onSubmit: (name: string) => void;
  }

  let { open = $bindable(), initial, onSubmit }: Props = $props();

  /** Every opening starts from the name as it is now; what is typed then is the field's. */
  let name = $derived(open ? initial : '');

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    // Read before closing: closed, the field starts over.
    const chosen = name.trim();
    open = false;
    onSubmit(chosen);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content data-testid="diary-name-dialog">
    <form class="grid gap-4" onsubmit={submit}>
      <Dialog.Header>
        <Dialog.Title>{m.diary_rename()}</Dialog.Title>
      </Dialog.Header>
      <Input
        bind:value={name}
        aria-label={m.diary_name()}
        placeholder={m.diary_untitled()}
        maxlength={255}
        autocomplete="off"
        enterkeyhint="done"
        class="roomy:h-touch roomy:text-base"
      />
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
        <Button type="submit" class="roomy:min-h-touch">
          {m['Categories.CreateCategoryDialog.UpdateText']()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
