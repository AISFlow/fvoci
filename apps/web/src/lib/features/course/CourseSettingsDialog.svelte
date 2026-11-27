<!--
  "Edit course": its icon, name and description, saved with undo when changed.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import ShuffleIcon from '@lucide/svelte/icons/shuffle';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { randomEmoji } from '$lib/logic/random-emoji.ts';
  import type { Container } from '$lib/logic/wire.ts';
  import type { CourseLooks } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Textarea } from '$lib/ui/textarea/index.js';

  interface Props {
    open: boolean;
    course: Pick<Container, 'title' | 'description' | 'icon'>;
    onSave: (looks: CourseLooks) => void;
  }

  let { open = $bindable(), course, onSave }: Props = $props();

  const uid = $props.id();
  let title = $state('');
  let description = $state('');
  let icon = $state('');

  function opened(): void {
    untrack(() => {
      title = course.title;
      description = course.description;
      icon = course.icon ?? '';
    });
  }

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    open = false;
    const looks: CourseLooks = {
      ...(title.trim() === course.title ? {} : { title: title.trim() }),
      ...(description === course.description ? {} : { description }),
      ...(icon === (course.icon ?? '') ? {} : { icon }),
    };
    if (Object.keys(looks).length > 0) onSave(looks);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    class="max-h-[calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard))] overflow-y-auto"
    data-testid="course-settings"
    {@attach opened}
  >
    <form class="grid gap-4" onsubmit={submit}>
      <Dialog.Header>
        <Dialog.Title>{m.course_settings_title()}</Dialog.Title>
      </Dialog.Header>
      <div class="flex items-center gap-2">
        <span
          class="grid size-10 shrink-0 place-items-center rounded-md border text-2xl"
          role="img"
          aria-label={icon ? m.wiki_icon_is({ icon }) : m.wiki_no_icon()}
          data-testid="course-settings-icon">{icon}</span
        >
        <IconButton
          label={m['ViewTitle.random-icon']()}
          data-testid="course-settings-random-icon"
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
        <label for="{uid}-name" class="font-medium">{m.course_name()}</label>
        <Input
          id="{uid}-name"
          bind:value={title}
          maxlength={255}
          autocomplete="off"
          enterkeyhint="done"
          data-testid="course-settings-name"
          class="roomy:h-touch roomy:text-base"
        />
      </div>
      <div class="grid gap-1.5">
        <label for="{uid}-description" class="font-medium">{m.course_description()}</label>
        <Textarea
          id="{uid}-description"
          bind:value={description}
          rows={3}
          data-testid="course-settings-description"
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
        <Button type="submit" class="roomy:min-h-touch" data-testid="course-settings-save">
          {m.wiki_save()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
