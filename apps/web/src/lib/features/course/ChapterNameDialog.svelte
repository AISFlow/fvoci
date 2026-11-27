<!--
  Create or rename a chapter of a course. Chapters are title-only groupings in
  the outline, so they are named here rather than in a body view.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';

  interface Props {
    open: boolean;
    mode: 'create' | 'rename';
    initial?: string;
    onSubmit: (title: string) => void;
  }

  let { open = $bindable(), mode, initial = '', onSubmit }: Props = $props();

  let title = $state('');

  function opened(): void {
    untrack(() => {
      title = initial;
    });
  }

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    open = false;
    onSubmit(title.trim());
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content data-testid="chapter-name-dialog" {@attach opened}>
    <form class="grid gap-4" onsubmit={submit}>
      <Dialog.Header>
        <Dialog.Title>
          {mode === 'create' ? m.course_new_chapter_title() : m.course_rename_chapter()}
        </Dialog.Title>
      </Dialog.Header>
      <Input
        bind:value={title}
        aria-label={m.course_chapter_name()}
        placeholder={m.course_chapter_name()}
        maxlength={255}
        autocomplete="off"
        enterkeyhint="done"
        data-testid="chapter-name-input"
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
        <Button type="submit" class="roomy:min-h-touch" data-testid="chapter-name-submit">
          {mode === 'create' ? m['Categories.CreateCategoryDialog.CreateText']() : m.wiki_save()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
