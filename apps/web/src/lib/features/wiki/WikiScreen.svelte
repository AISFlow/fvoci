<!--
  A wiki: its pages as a tree at the left, the page shown at the right.

  Where the pane has room for two columns the tree is one of them. In a
  narrow pane (a phone) the page has the whole width, and the tree is a
  drawer over it, opened by the button at the head of the page and closed by
  choosing a page, by Escape, by a tap beside it and by "back", as the
  sidebar's drawer is.

  A wiki without pages says so, with a way to make the first one for whoever
  may write in it. A page made here is shown with the caret in its title, the
  page it went below opened in the tree. The pages above the page shown are
  open, so that it is in sight.

  The page's headings are its outline (`PageOutline`): beside the page where
  there is room, otherwise behind a button in the bar at the head of the page.
  Where it would be behind the button, the bar's place is kept above a page
  with too few headings for one, so that the page does not move down and up
  as a second heading is written and taken out; it stays at the head as the
  page scrolls only with something in it.
-->
<script lang="ts">
  import { tick, untrack, type Snippet } from 'svelte';
  import type { Attachment } from 'svelte/attachments';

  import FilePlusIcon from '@lucide/svelte/icons/file-plus';
  import ListTreeIcon from '@lucide/svelte/icons/list-tree';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { focusRevealed } from '$lib/focus.ts';
  import { coverPane } from '$lib/layout/over-pane.svelte.ts';
  import { pane } from '$lib/layout/room.svelte.ts';
  import type { PagePlace } from '$lib/logic/page-moves.ts';
  import { ancestorsOf, PAGE_TITLE_MAX } from '$lib/logic/page-tree.ts';
  import {
    whileHere,
    type PageDraft,
    type PageStore,
    type WikiPage,
    type WikiStore,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import MovePageDialog from './MovePageDialog.svelte';
  import OutlineButton from './OutlineButton.svelte';
  import OutlineColumns from './OutlineColumns.svelte';
  import { PageOutline } from './page-outline.svelte.ts';
  import PageTree from './PageTree.svelte';
  import WikiHeader from './WikiHeader.svelte';
  import PageView from './PageView.svelte';
  import { TreeOpen } from './tree-open.svelte.ts';

  interface Props {
    wiki: WikiStore;
    page: PageStore | undefined;
    pageHref: (pageId: string) => string;
    onOpenPage: (pageId: string) => void;
    /** What a page that is not there shows. */
    missingPage: Snippet;
    /**
     * The body of the page shown, and what goes below it; the body alone, and
     * nothing else, is put in an element with `outlined` attached (its headings).
     */
    body: Snippet<[PageStore, Attachment<HTMLElement>]>;
    /** What the head of the page shown offers to do with it, beside who is on it. */
    pageActions?: Snippet<[PageStore]> | undefined;
    /** Asks to delete a page; left out where pages are not deleted here. */
    onDelete?: ((page: WikiPage) => void) | undefined;
    /** Asks to edit the wiki's name, description and icon (its managers'). */
    onEditWiki: () => void;
    /** Asks to delete the wiki (its managers'). */
    onDeleteWiki: () => void;
    /** Asks to show the wiki's deleted pages (its editors'). */
    onOpenTrash: () => void;
    /** Asks to show the wiki's people (`ShareDialog`). */
    onShare: () => void;
    /** Something to tell at the head of the page (a page shown was deleted). */
    notice?: string | undefined;
    onDismissNotice?: (() => void) | undefined;
    /** Take the focus when shown: the reader just joined, from a button that is gone. */
    takeFocus?: boolean;
  }

  let {
    wiki,
    page,
    pageHref,
    onOpenPage,
    missingPage,
    body,
    pageActions,
    onDelete,
    onEditWiki,
    onDeleteWiki,
    onOpenTrash,
    onShare,
    notice,
    onDismissNotice,
    takeFocus = false,
  }: Props = $props();

  const uid = $props.id();
  const open = $derived(new TreeOpen(wiki.workspace.me?.id ?? '', wiki.wikiId));
  const shown = $derived(page ? (wiki.page(page.pageId) ?? page.meta) : undefined);
  /** A page is shown (and with it what is told at its head, beside its outline). */
  const reading = $derived(!page?.missing && shown !== undefined);
  /** The pages above the page shown, as one key: it changes with the page, not with a rename. */
  const above = $derived(page ? ancestorsOf(wiki.pages, page.pageId).join(' ') : '');
  $effect(() => {
    const ids = above;
    const target = open;
    if (ids !== '')
      untrack(() => {
        target.set(ids.split(' '));
      });
  });

  /** The headings of the page shown. */
  const outline = new PageOutline();

  /** The page made last here: shown with the caret in its title. */
  let made = $state<string | undefined>(undefined);
  /** The tree is a drawer over the page: a narrow pane. */
  const narrow = $derived(pane.measured && !pane.columns);
  /** The wiki and the page on the screen, as one key. */
  const place = $derived(`${wiki.wikiId} ${page?.pageId ?? ''}`);
  /**
   * Whether the drawer was asked out. It is out over the place it was opened over
   * only: when the screen shows another (a page that was deleted elsewhere, an
   * address followed), the drawer is away, and stays away when that place is back.
   */
  // Away over every place: a function of the place only so that the derived, which is
  // written when the drawer is asked out, is made anew (away) when the place changes.
  const away = (_over: string): boolean => false;
  let drawer = $derived(away(place));
  const drawerOut = $derived(narrow && drawer);
  coverPane(() => drawerOut);
  let panel = $state<HTMLElement>();
  let opener = $state<HTMLElement | null>(null);

  /** Opens the drawer with the focus on the tree's page in the tab order (none: its close button). */
  async function openDrawer(): Promise<void> {
    drawer = true;
    await tick();
    const target =
      panel?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]') ??
      panel?.querySelector<HTMLElement>('button');
    focusRevealed(target);
  }

  /** Puts the drawer away, the focus back on the button that opened it once it is no longer inert. */
  async function closeDrawer(): Promise<void> {
    drawer = false;
    await tick();
    focusRevealed(opener);
  }

  closeOnBack(
    () => drawerOut,
    () => {
      drawer = false;
    },
  );

  /** Shows a page; chosen in the drawer, the drawer goes and its button has the focus. */
  function show(pageId: string): void {
    if (drawerOut) void closeDrawer();
    made = undefined;
    onOpenPage(pageId);
  }

  /** A page is being made: a second click does not make another. */
  let creating = $state(false);
  /** The page "Move…" asks a place for. */
  let moving = $state<string | undefined>(undefined);
  let movingOpen = $state(false);

  /** Moves a page; the page it goes below unfolds, so that it stays in sight. */
  function move(pageId: string, to: PagePlace): Promise<boolean> {
    if (to.parentId !== '') open.set([to.parentId, ...ancestorsOf(wiki.pages, to.parentId)]);
    return wiki.movePage(pageId, to);
  }

  /**
   * Copies a page with those below it, and shows the copy, right after it,
   * open if the page is (the copies below it folded). Gives the copy's title,
   * to be told; undefined when it was refused (told).
   */
  async function duplicate(source: WikiPage): Promise<string | undefined> {
    const copying = wiki;
    // The longest title cut to leave the words of a copy room.
    const room = PAGE_TITLE_MAX - Array.from(m.wiki_page_copy({ title: '' })).length;
    const base = Array.from(source.title || m.wiki_page_untitled())
      .slice(0, room)
      .join('');
    const title = m.wiki_page_copy({ title: base });
    const copy = await whileHere(copying.workspace, () => copying.duplicatePage(source.id, title));
    if (!copy || wiki !== copying) return undefined;
    if (open.has(source.id)) open.set([copy.id]);
    show(copy.id);
    return copy.title;
  }

  async function create(draft: PageDraft): Promise<void> {
    if (creating) return;
    creating = true;
    const making = wiki;
    try {
      const madePage = await whileHere(making.workspace, () => making.createPage(draft));
      // Shown only while its wiki is: the reader may have gone on to another meanwhile.
      if (!madePage || wiki !== making) return;
      if (madePage.parentId) open.set([madePage.parentId]);
      if (drawerOut) drawer = false;
      made = undefined;
      onOpenPage(madePage.id);
      // The page made takes the caret in its title (`PageView`), not the drawer's button.
      made = madePage.id;
    } finally {
      creating = false;
    }
  }

  /** The places of the drawer the Tab key stops at, in order. */
  const stops = (within: HTMLElement): HTMLElement[] =>
    Array.from(
      within.querySelectorAll<HTMLElement>(
        'button:not([tabindex="-1"]):not(:disabled), a[href]:not([tabindex="-1"]), [tabindex="0"]',
      ),
    );

  function onkeydown(event: KeyboardEvent): void {
    if (!drawerOut || event.defaultPrevented || !panel) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      void closeDrawer();
    } else if (event.key === 'Tab') {
      // A modal drawer: Tab goes round inside it.
      const inside = stops(panel);
      const [first, last] = [inside[0], inside.at(-1)];
      const at = document.activeElement;
      if (!event.shiftKey && at === last) {
        event.preventDefault();
        first?.focus();
      } else if (event.shiftKey && at === first) {
        event.preventDefault();
        last?.focus();
      }
    }
  }
</script>

<div
  class="relative flex h-full min-h-0 outline-none"
  tabindex="-1"
  data-testid="wiki-screen"
  data-narrow={narrow ? '' : undefined}
  {@attach (element: HTMLElement) => {
    if (takeFocus) element.focus();
  }}
>
  {#if drawerOut}
    <button
      type="button"
      tabindex="-1"
      aria-label={m.wiki_close_tree()}
      data-testid="page-tree-scrim"
      class="absolute inset-0 z-20 animate-in cursor-default scrim duration-200 fade-in-0"
      onclick={() => void closeDrawer()}
    ></button>
  {/if}

  <!-- The tree: a column beside the page, or a drawer over it, kept in the document when away. -->
  <div
    id="{uid}-tree"
    bind:this={panel}
    data-testid="page-tree-panel"
    role={drawerOut ? 'dialog' : undefined}
    aria-modal={drawerOut ? 'true' : undefined}
    aria-label={drawerOut ? m.wiki_pages() : undefined}
    inert={narrow && !drawer}
    class={[
      'flex min-h-0 flex-col bg-background',
      narrow
        ? 'absolute inset-y-0 left-0 z-30 w-[min(18rem,85%)] border-r shadow-lg'
        : 'w-64 shrink-0 border-r',
      narrow &&
        (drawer
          ? '[transition:translate_200ms_ease-out]'
          : 'invisible -translate-x-full [transition:translate_200ms_ease-in,visibility_0s_200ms]'),
    ]}
    {onkeydown}
  >
    {#if drawerOut}
      <div class="flex shrink-0 justify-end px-1 pt-1">
        <IconButton label={m.wiki_close_tree()} onclick={() => void closeDrawer()}>
          <XIcon />
        </IconButton>
      </div>
    {/if}
    <WikiHeader
      {wiki}
      onEdit={onEditWiki}
      onDelete={onDeleteWiki}
      onTrash={onOpenTrash}
      {onShare}
    />
    <PageTree
      {wiki}
      currentId={page?.pageId}
      {open}
      {pageHref}
      onOpenPage={show}
      onCreate={(draft: PageDraft) => void create(draft)}
      {onDelete}
      onMove={move}
      onMoveTo={(chosen: WikiPage) => {
        [moving, movingOpen] = [chosen.id, true];
      }}
      onDuplicate={duplicate}
      busy={creating}
    />
  </div>

  <div
    class="min-w-0 flex-1 overflow-y-auto overscroll-contain"
    inert={drawerOut}
    {@attach outline.follow}
  >
    {#if narrow || outline.mode === 'button' || (reading && outline.where === 'button')}
      <div
        class={[
          'top-0 z-10 flex min-h-9 items-center gap-1 px-2 py-1 roomy:min-h-[calc(var(--spacing-touch)+0.5rem)]',
          (narrow || outline.mode === 'button') && 'sticky bg-background/95',
        ]}
        {@attach outline.bar}
      >
        {#if narrow}
          <Button
            bind:ref={opener}
            variant="ghost"
            size="sm"
            class="text-muted-foreground roomy:min-h-touch"
            data-testid="open-page-tree"
            aria-haspopup="dialog"
            aria-expanded={drawer}
            aria-controls="{uid}-tree"
            onclick={() => void openDrawer()}
          >
            <ListTreeIcon aria-hidden="true" />
            {m.wiki_pages()}
          </Button>
        {/if}
        <div class="min-w-0 flex-1"></div>
        {#if outline.mode === 'button'}
          <OutlineButton {outline} id="{uid}-outline" />
        {/if}
      </div>
    {/if}
    {#if notice && !reading}{@render told(notice)}{/if}
    {#if wiki.pagesReady && wiki.pages.length === 0}
      <div class="grid h-full place-items-center p-6 text-center" data-testid="wiki-empty">
        <div class="flex max-w-md flex-col items-center gap-3">
          <p class="text-muted-foreground">{m.wiki_no_pages()}</p>
          {#if wiki.canEdit}
            <Button
              class="roomy:min-h-touch"
              data-testid="wiki-first-page"
              disabled={creating}
              onclick={() => void create({})}
            >
              <FilePlusIcon aria-hidden="true" />
              {m.wiki_first_page()}
            </Button>
          {/if}
        </div>
      </div>
    {:else if page?.missing}
      {@render missingPage()}
    {:else if page && shown}
      <OutlineColumns {outline} id="{uid}-outline">
        {#if notice}{@render told(notice)}{/if}
        {#key page}
          <PageView
            {wiki}
            store={page}
            page={shown}
            autofocus={made === page.pageId}
            body={outlinedBody}
            actions={pageActions}
          />
        {/key}
      </OutlineColumns>
    {:else if !wiki.pagesReady}
      <div class="grid h-full place-items-center p-6">
        <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
      </div>
    {/if}
  </div>
</div>

{#snippet told(text: string)}
  <div
    class="mx-auto mt-4 flex max-w-3xl items-center gap-2 rounded-lg border bg-muted px-3 py-2"
    role="status"
    data-testid="wiki-notice"
  >
    <p class="min-w-0 flex-1">{text}</p>
    {#if onDismissNotice}
      <IconButton label={m.ui_close()} onclick={onDismissNotice}>
        <XIcon />
      </IconButton>
    {/if}
  </div>
{/snippet}

{#snippet outlinedBody(store: PageStore)}
  {@render body(store, outline.source)}
{/snippet}

<MovePageDialog
  bind:open={movingOpen}
  {wiki}
  pageId={moving}
  onMove={(pageId: string, to: PagePlace) => void move(pageId, to)}
/>
