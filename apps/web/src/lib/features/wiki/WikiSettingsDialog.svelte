<!--
  "Edit wiki": its icon, its name and its description, as they are when the
  dialog opens. The icon is one emoji, drawn at random or taken away. Only
  what was changed is saved (`onSave`), with undo; Enter in the name saves.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import ShuffleIcon from '@lucide/svelte/icons/shuffle';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { randomEmoji } from '$lib/logic/random-emoji.ts';
  import type { Container } from '$lib/logic/wire.ts';
  import type { WikiLooks } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Textarea } from '$lib/ui/textarea/index.js';

  interface Props {
    open: boolean;
    wiki: Pick<Container, 'title' | 'description' | 'icon'>;
    onSave: (looks: WikiLooks) => void;
  }

  let { open = $bindable(), wiki, onSave }: Props = $props();

  const uid = $props.id();
  let title = $state('');
  let description = $state('');
  let icon = $state('');

  /** Each opening starts from the wiki as it is then; a change of it meanwhile keeps what is typed. */
  function opened(): void {
    untrack(() => {
      title = wiki.title;
      description = wiki.description;
      icon = wiki.icon ?? '';
    });
  }

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    open = false;
    const looks: WikiLooks = {
      ...(title.trim() === wiki.title ? {} : { title: title.trim() }),
      ...(description === wiki.description ? {} : { description }),
      ...(icon === (wiki.icon ?? '') ? {} : { icon }),
    };
    if (Object.keys(looks).length > 0) onSave(looks);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    class="max-h-[calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard))] overflow-y-auto"
    data-testid="wiki-settings"
    {@attach opened}
  >
    <form class="grid gap-4" onsubmit={submit}>
      <Dialog.Header>
        <Dialog.Title>{m.wiki_settings_title()}</Dialog.Title>
      </Dialog.Header>
      <div class="flex items-center gap-2">
        <span
          class="grid size-10 shrink-0 place-items-center rounded-md border text-2xl"
          role="img"
          aria-label={icon ? m.wiki_icon_is({ icon }) : m.wiki_no_icon()}
          data-testid="wiki-settings-icon">{icon}</span
        >
        <IconButton
          label={m['ViewTitle.random-icon']()}
          data-testid="wiki-settings-random-icon"
          onclick={() => {
            icon = randomEmoji();
          }}
        >
          <ShuffleIcon />
        </IconButton>
        {#if icon}
          <IconButton
            label={m['ViewTitle.remove-icon']()}
            onclick={() => {
              icon = '';
            }}
          >
            <XIcon />
          </IconButton>
        {/if}
      </div>
      <div class="grid gap-1.5">
        <label for="{uid}-name" class="font-medium">{m.wiki_name()}</label>
        <Input
          id="{uid}-name"
          bind:value={title}
          maxlength={255}
          autocomplete="off"
          enterkeyhint="done"
          data-testid="wiki-settings-name"
          class="roomy:h-touch roomy:text-base"
        />
      </div>
      <div class="grid gap-1.5">
        <label for="{uid}-description" class="font-medium">{m.wiki_description()}</label>
        <Textarea
          id="{uid}-description"
          bind:value={description}
          rows={3}
          data-testid="wiki-settings-description"
          class="roomy:text-base"
        />
      </div>
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
        <Button type="submit" class="roomy:min-h-touch" data-testid="wiki-settings-save">
          {m.wiki_save()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
