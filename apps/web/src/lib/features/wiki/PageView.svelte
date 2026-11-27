<!--
  A page of a wiki: its icon, its title, who else is on it now, and its body.

  The title is a text box that grows with its text, as a card's: one
  paragraph, stored when the typing pauses and when the field is left; Enter
  ends the editing and Escape puts the stored title back. While the reader
  types, a title arriving from somebody else waits. The icon is one emoji: a
  page without one offers "Add icon" (one at random), and the icon's menu
  draws another or takes it away. Whoever may only read the wiki sees the
  title and the icon as text.

  The body is the caller's (`body`): the page's room is opened by what is put
  there; so is what the head offers to do with the page (`actions`).
-->
<script lang="ts">
  import ShuffleIcon from '@lucide/svelte/icons/shuffle';
  import SmilePlusIcon from '@lucide/svelte/icons/smile-plus';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import { onDestroy, untrack, type Snippet } from 'svelte';

  import { autosize } from '$lib/components/autosize.ts';
  import { isComposing } from '$lib/keys.ts';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { randomEmoji } from '$lib/logic/random-emoji.ts';
  import type { PageStore, WikiPage, WikiStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import PageViewers from './PageViewers.svelte';

  interface Props {
    wiki: WikiStore;
    store: PageStore;
    page: WikiPage;
    /** Start with the caret in the title: for a page that was just made. */
    autofocus?: boolean;
    body: Snippet<[PageStore]>;
    actions?: Snippet<[PageStore]> | undefined;
  }

  let { wiki, store, page, autofocus = false, body, actions }: Props = $props();

  /** How long the typing has to pause before the title is stored, in milliseconds. */
  const PAUSE = 800;

  const editable = $derived(wiki.canEdit);

  let field = $state<HTMLTextAreaElement>();
  let draft = $state(untrack(() => page.title));
  let editing = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  /** What `save` reads when the component is being removed: not reactive on purpose. */
  let unsaved: string | undefined;

  // A title that changes elsewhere replaces the text, unless the reader is typing.
  $effect(() => {
    const stored = page.title;
    if (!untrack(() => editing)) draft = stored;
  });

  function save(): void {
    clearTimeout(timer);
    timer = undefined;
    if (unsaved === undefined) return;
    const title = unsaved.trim();
    unsaved = undefined;
    if (title !== page.title) void wiki.changePage(store.pageId, { title });
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
      event.preventDefault();
      clearTimeout(timer);
      unsaved = undefined;
      draft = page.title;
      field?.blur();
    }
  }

  function onblur(): void {
    editing = false;
    const typed = unsaved !== undefined;
    save();
    // Nothing typed: the title as it is now, which somebody else may have changed meanwhile.
    draft = typed ? draft.trim() : page.title;
  }

  // A page just made takes the caret in its title, also when it was shown before it was known to be new.
  $effect(() => {
    const target = field;
    if (autofocus && editable && target)
      untrack(() => {
        target.focus();
      });
  });

  // Removed while being edited (another page was opened): keep what was typed.
  onDestroy(save);

  const setIcon = (icon: string): void => {
    void wiki.changePage(store.pageId, { icon });
  };

  const iconActions = $derived<MenuAction[]>([
    {
      id: 'random-icon',
      label: m['ViewTitle.random-icon'](),
      icon: ShuffleIcon,
      run: () => {
        setIcon(randomEmoji());
      },
    },
    {
      id: 'remove-icon',
      label: m['ViewTitle.remove-icon'](),
      icon: Trash2Icon,
      run: () => {
        setIcon('');
      },
    },
  ]);

  const TITLE = 'w-full min-w-0 text-2xl font-semibold tracking-tight @columns/pane:text-3xl';
</script>

<article
  class="mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6 @columns/pane:px-8"
  aria-label={page.title || m.wiki_page_untitled()}
  data-testid="page-view"
  data-page={page.id}
>
  <header class="flex flex-col items-start gap-1" data-testid="page-heading">
    <div class="flex w-full items-center gap-2">
      {#if page.icon !== '' && editable}
        <ActionMenu
          title={m.wiki_icon_change()}
          subtitle={page.title || m.wiki_page_untitled()}
          align="start"
          actions={iconActions}
        >
          {#snippet trigger(props: Record<string, unknown>)}
            <button
              {...props}
              type="button"
              aria-label={m.wiki_icon_change()}
              data-testid="page-icon"
              class="grid size-12 place-items-center rounded-md text-4xl leading-none hover:bg-muted aria-expanded:bg-muted roomy:size-touch"
            >
              <span aria-hidden="true">{page.icon}</span>
            </button>
          {/snippet}
        </ActionMenu>
      {:else if page.icon !== ''}
        <span class="grid size-12 place-items-center text-4xl leading-none" data-testid="page-icon">
          <span aria-hidden="true">{page.icon}</span>
        </span>
      {:else if editable}
        <Button
          variant="ghost"
          class="-ml-2 text-muted-foreground roomy:min-h-touch"
          data-testid="page-add-icon"
          onclick={() => {
            setIcon(randomEmoji());
          }}
        >
          <SmilePlusIcon data-icon="inline-start" aria-hidden="true" />
          {m['CardDetail.add-icon']()}
        </Button>
      {/if}
      <div class="min-w-0 flex-1"></div>
      <PageViewers {wiki} pageId={page.id} large />
      {@render actions?.(store)}
    </div>
    {#if editable}
      <textarea
        bind:this={field}
        bind:value={draft}
        rows="1"
        data-testid="page-title"
        aria-label={m.wiki_page_title()}
        placeholder={m.wiki_page_untitled()}
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
      <h1 class={[TITLE, page.title === '' && 'text-muted-foreground']} data-testid="page-title">
        {page.title || m.wiki_page_untitled()}
      </h1>
    {/if}
  </header>
  {@render body(store)}
</article>
