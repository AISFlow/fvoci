<!--
  The window of new things, one layer: a title (may stay empty) and, in a
  shared space, who finds it (open by default), asked once at the top; then a
  card per kind (board, wiki, course, and in the reader's own space a diary,
  another volume each time) that makes it at once, and the templates by kind
  below. The templates are read each time the window opens: the workspace's
  (which can be opened to be changed) and then the built-in ones.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import type { Container, ContainerVisibility } from '$lib/logic/wire.ts';
  import type { ContainerDraft, NewKind, Workspace } from '$lib/state/index.ts';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';

  import NewKindCards from './NewKindCards.svelte';
  import NewVisibility from './NewVisibility.svelte';
  import TemplateRows from './TemplateRows.svelte';

  interface Props {
    open: boolean;
    workspace: Workspace;
    /** The reader's own space: nobody to open it to, and diaries to keep. */
    alone: boolean;
    workspaceName: string;
    onCreate: (kind: NewKind, draft: ContainerDraft) => void;
    onFromTemplate: (template: Container, draft: ContainerDraft) => void;
    /** Opens a template of the workspace on its page, to be changed. */
    onEditTemplate: (template: Container) => void;
  }

  let {
    open = $bindable(),
    workspace,
    alone,
    workspaceName,
    onCreate,
    onFromTemplate,
    onEditTemplate,
  }: Props = $props();

  const compact = $derived(page.stage === 'compact');
  const uid = $props.id();
  let title = $state('');
  let visibility = $state<ContainerVisibility>('open');
  let templates = $state.raw<Container[] | undefined>(undefined);
  /** Readings of the templates begun: only the latest is shown. */
  let readings = 0;

  const kinds = $derived<NewKind[]>([
    'board',
    ...(workspace.backend.wiki ? (['wiki'] as const) : []),
    ...(workspace.backend.course ? (['course'] as const) : []),
    ...(alone && workspace.backend.diary ? (['diary'] as const) : []),
  ]);

  closeOnBack(
    () => open,
    () => {
      open = false;
    },
  );

  /** Each time the window opens (its content is made): a fresh title, and the templates read anew. */
  function opened(): void {
    untrack(() => {
      title = '';
      visibility = 'open';
      templates = undefined;
      readings += 1;
      const reading = readings;
      void workspace.templates().then((read) => {
        if (reading === readings) templates = read;
      });
    });
  }

  /** Where the cards of the kinds are: Enter in the title goes on to the first. */
  let kindsArea = $state<HTMLElement>();

  /**
   * Enter in the title is done with it: on to the cards of the kinds, one of which makes the
   * thing (and on a phone the keyboard goes). Not while a word is being composed.
   */
  function onTitleKey(event: KeyboardEvent): void {
    if (event.key !== 'Enter' || event.isComposing) return;
    event.preventDefault();
    kindsArea?.querySelector<HTMLElement>('button')?.focus();
  }

  function draft(): ContainerDraft {
    return { title: title.trim(), visibility: alone ? 'private' : visibility };
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    {@attach opened}
    data-testid="template-picker"
    showCloseButton={false}
    class={[
      'flex flex-col gap-0 p-0',
      compact
        ? 'top-0 left-0 h-dvh max-w-none translate-x-0 translate-y-0 rounded-none pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none'
        : 'top-[calc(50%-var(--keyboard)/2)] max-h-[calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard))] w-[min(36rem,calc(100%-2rem))] max-w-none sm:max-w-none',
    ]}
  >
    <Dialog.Header class="shrink-0 flex-row items-start gap-2 border-b py-2 pr-2 pl-4">
      <div class="min-w-0 flex-1 py-1">
        <Dialog.Title class="text-base font-semibold">
          {m['BoardTemplateSelector.title']()}
        </Dialog.Title>
        <Dialog.Description class="text-muted-foreground">
          {m['BoardTemplateSelector.description']()}
        </Dialog.Description>
      </div>
      <Dialog.Close>
        {#snippet child({ props }: { props: Record<string, unknown> })}
          <IconButton {...props} label={m.ui_close()}>
            <XIcon />
          </IconButton>
        {/snippet}
      </Dialog.Close>
    </Dialog.Header>

    <!--
      Full screen, its foot is under the keyboard: the list keeps room for it there. The window
      takes the keyboard off its height and is centred above it, so its foot is clear of it.
    -->
    <div
      class={[
        'grid min-h-0 flex-1 content-start gap-4 overflow-y-auto overscroll-contain p-4',
        compact && 'pb-[calc(1rem+var(--keyboard))]',
      ]}
    >
      <div class="grid gap-1.5">
        <label for="{uid}-title" class="text-sm font-medium">{m.new_title_label()}</label>
        <Input
          id="{uid}-title"
          bind:value={title}
          placeholder={m.new_title_placeholder()}
          maxlength={255}
          autocomplete="off"
          enterkeyhint="done"
          onkeydown={onTitleKey}
          data-testid="new-title"
          class="text-base roomy:h-touch"
        />
      </div>
      {#if !alone}
        <NewVisibility bind:visibility {workspaceName} />
      {/if}
      <div bind:this={kindsArea} class="contents">
        <NewKindCards
          {kinds}
          onPick={(kind: NewKind) => {
            const asked = draft();
            open = false;
            onCreate(kind, asked);
          }}
        />
      </div>
      {#if templates === undefined}
        <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
      {:else}
        <TemplateRows
          {templates}
          locale={workspace.locale}
          onUse={(template: Container) => {
            const asked = draft();
            open = false;
            onFromTemplate(template, asked);
          }}
          onEdit={(template: Container) => {
            open = false;
            onEditTemplate(template);
          }}
        />
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>
