<!--
  Asks for the name of a category: of a new one, or of one that is renamed.
  The field starts with the name as it is and holds the focus; Enter submits.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';

  interface Props {
    open: boolean;
    /** The name the field starts with: empty for a new category. */
    initial: string;
    /** A new category is made, or an existing one is renamed: decides the words. */
    mode: 'create' | 'rename';
    onSubmit: (name: string) => void;
  }

  let { open = $bindable(), initial, mode, onSubmit }: Props = $props();

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
  <Dialog.Content data-testid="category-dialog">
    <form class="grid gap-4" onsubmit={submit}>
      <Dialog.Header>
        <Dialog.Title>
          {mode === 'create'
            ? m['SidebarCategories.CategoryMenu.CreateNew']()
            : m['SidebarCategories.CategoryMenu.Update']()}
        </Dialog.Title>
      </Dialog.Header>
      <Input
        bind:value={name}
        aria-label={m.shell_category_name()}
        placeholder={m['Categories.CreateCategoryDialog.Placeholder']()}
        maxlength={100}
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
        <Button type="submit" class="roomy:min-h-touch" disabled={trimmed === ''}>
          {mode === 'create'
            ? m['Categories.CreateCategoryDialog.CreateText']()
            : m['Categories.CreateCategoryDialog.UpdateText']()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
