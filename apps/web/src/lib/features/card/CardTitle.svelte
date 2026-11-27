<!--
  The icon and the title of a card.

  The title is a text box that grows with its text and wraps like a heading.
  It is one paragraph: Enter ends the editing instead of starting a line, and
  line breaks that are pasted in become spaces. What is typed is stored when
  the typing pauses and when the field is left; Escape puts the stored title
  back. While the reader types, a title arriving from somebody else waits.

  The icon is one emoji. A card without one offers "Add icon", which gives it
  one at random; the icon's menu draws another or takes it away. Choosing a
  particular emoji needs the picker that is being built elsewhere (see
  icons.ts).
-->
<script lang="ts">
  import ShuffleIcon from '@lucide/svelte/icons/shuffle';
  import SmilePlusIcon from '@lucide/svelte/icons/smile-plus';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import { onDestroy, onMount, untrack, type Snippet } from 'svelte';

  import { isComposing } from '$lib/keys.ts';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BoardStore, CardSummary } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { autosize } from '$lib/components/autosize.ts';
  import { randomEmoji } from './icons.ts';

  interface Props {
    board: BoardStore;
    card: CardSummary;
    /** Start with the caret in the title: for a card that was just made. */
    autofocus?: boolean;
    /** Optional inline controls beside the icon / "Add icon" row. */
    meta?: Snippet;
  }

  let { board, card, autofocus = false, meta }: Props = $props();

  /** How long the typing has to pause before the title is stored, in milliseconds. */
  const PAUSE = 800;

  const editable = $derived(board.canEdit);
  const icon = $derived(card.fields.icon ?? '');

  let field = $state<HTMLTextAreaElement>();
  let draft = $state(untrack(() => card.title));
  let editing = $state(false);
  let iconMenuOpen = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  /**
   * What `save` reads when the component is being removed. Not reactive on
   * purpose: state read at that moment is what it was before the change that
   * removes the component.
   */
  let unsaved: string | undefined;

  // A title that changes elsewhere replaces the text, unless the reader is typing.
  $effect(() => {
    const stored = card.title;
    if (!untrack(() => editing)) draft = stored;
  });

  function save(): void {
    clearTimeout(timer);
    timer = undefined;
    if (unsaved === undefined) return;
    const title = unsaved.trim();
    unsaved = undefined;
    if (title !== card.title) void board.setCardTitle(card.id, title);
  }

  function oninput(event: Event & { currentTarget: HTMLTextAreaElement }): void {
    // One paragraph: a line break that was pasted or dictated becomes a space.
    draft = event.currentTarget.value.replace(/\s*[\r\n]+\s*/g, ' ');
    unsaved = draft;
    clearTimeout(timer);
    timer = setTimeout(save, PAUSE);
  }

  function onkeydown(event: KeyboardEvent): void {
    // While an input method is composing (Hangul, kana), Enter and Escape are its own.
    if (isComposing(event)) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      field?.blur();
    } else if (event.key === 'Escape') {
      // Escape belongs to the field: it must not also close the card.
      event.preventDefault();
      event.stopPropagation();
      clearTimeout(timer);
      unsaved = undefined;
      draft = card.title;
      field?.blur();
    }
  }

  function onblur(): void {
    editing = false;
    save();
    draft = draft.trim();
  }

  onMount(() => {
    if (autofocus && editable) field?.focus();
  });

  // Removed while being edited (the card was closed or changed for another): keep what was typed.
  onDestroy(save);

  const iconActions = $derived<MenuAction[]>([
    {
      id: 'random-icon',
      label: m['ViewTitle.random-icon'](),
      icon: ShuffleIcon,
      run: () => void board.setCardIcon(card.id, randomEmoji()),
    },
    {
      id: 'remove-icon',
      label: m['ViewTitle.remove-icon'](),
      icon: Trash2Icon,
      run: () => void board.setCardIcon(card.id, ''),
    },
  ]);

  const TITLE = 'w-full min-w-0 text-xl font-semibold tracking-tight @card-roomy/card:text-2xl';
</script>

<div class="flex flex-col items-start gap-1" data-testid="card-heading">
  {#if icon !== '' || editable}
    <div class="flex w-full flex-wrap items-center gap-2">
      {#if icon !== '' && editable}
        <ActionMenu
          bind:open={iconMenuOpen}
          title={m.card_icon_change()}
          subtitle={card.title || m['KanbanCard.untitled']()}
          align="start"
          actions={iconActions}
        >
          {#snippet trigger(props: Record<string, unknown>)}
            <button
              {...props}
              type="button"
              aria-label={m.card_icon_change()}
              data-testid="card-icon"
              class="grid size-10 place-items-center rounded-md text-3xl leading-none hover:bg-muted aria-expanded:bg-muted roomy:size-touch"
            >
              <span aria-hidden="true">{icon}</span>
            </button>
          {/snippet}
        </ActionMenu>
      {:else if icon !== ''}
        <span class="grid size-10 place-items-center text-3xl leading-none" data-testid="card-icon">
          <span aria-hidden="true">{icon}</span>
        </span>
      {:else if editable}
        <Button
          variant="ghost"
          class="-ml-2 text-muted-foreground roomy:min-h-touch"
          data-testid="card-add-icon"
          onclick={() => void board.setCardIcon(card.id, randomEmoji())}
        >
          <SmilePlusIcon data-icon="inline-start" aria-hidden="true" />
          {m['CardDetail.add-icon']()}
        </Button>
      {/if}
      {@render meta?.()}
    </div>
  {/if}

  {#if editable}
    <textarea
      bind:this={field}
      bind:value={draft}
      rows="1"
      data-card-title
      data-testid="card-title"
      aria-label={m['default-properties.title']()}
      placeholder={m['KanbanCard.untitled']()}
      autocomplete="off"
      enterkeyhint="done"
      class={[
        TITLE,
        'field-sizing-content resize-none overflow-hidden rounded-md bg-transparent placeholder:text-muted-foreground roomy:min-h-touch',
      ]}
      onfocus={() => {
        editing = true;
      }}
      {oninput}
      {onkeydown}
      {onblur}
      {@attach autosize(() => draft)}></textarea>
  {:else}
    <h2 class={[TITLE, card.title === '' && 'text-muted-foreground']} data-testid="card-title">
      {card.title || m['KanbanCard.untitled']()}
    </h2>
  {/if}
</div>
