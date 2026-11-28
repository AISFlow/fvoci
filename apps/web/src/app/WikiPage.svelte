<!--
  One wiki: its pages as a tree and the page the address names. Which wiki
  and page comes from the address; this keeps the stores and the address in
  step.

  A wiki's address without a page shows its first page, the address put in
  place (no entry of the history). A wiki not there for the reader, a board's
  identifier at a wiki's address, and a page that is not there show the same
  "not found" as the rest of the app; one for the invited only that the
  reader manages without reading it shows a way to its members and to join
  it (`ManagedContainer`). An open wiki the reader has not joined
  is read as its viewers read it, under a bar to join it and write in it.

  The page's body is the card's kind of body (`CardBody`), edited together in
  the page's room; while the wiki is shown the reader is on it, and on the
  page shown (the wiki's presence), as on a board and its card. Below the body
  are the page's comments, the card's (`CardComments`, `CommentComposer`),
  and the page's menu opens the versions of its body, the card's dialog
  (`BodyVersions`).
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { Attachment } from 'svelte/attachments';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import BodyVersions from '$lib/features/card/BodyVersions.svelte';
  import {
    keepVersion,
    pageVersions,
    VersionsReading,
  } from '$lib/features/card/body-versions.svelte.ts';
  import ShareDialog from '$lib/features/shell/ShareDialog.svelte';
  import { bodyReferences } from '$lib/features/card/body-references.ts';
  import CardBacklinks from '$lib/features/card/CardBacklinks.svelte';
  import CardBody from '$lib/features/card/CardBody.svelte';
  import CardComments from '$lib/features/card/CardComments.svelte';
  import CommentReactions from '$lib/features/card/CommentReactions.svelte';
  import CommentComposer from '$lib/features/card/CommentComposer.svelte';
  import FavoriteToggle from '$lib/components/favorite-toggle/FavoriteToggle.svelte';
  import WatchToggle from '$lib/features/card/WatchToggle.svelte';
  import PageMenu from '$lib/features/wiki/PageMenu.svelte';
  import TrashDialog from '$lib/features/shell/TrashDialog.svelte';
  import WikiSettingsDialog from '$lib/features/wiki/WikiSettingsDialog.svelte';
  import WikiJoin from '$lib/features/wiki/WikiJoin.svelte';
  import WikiScreen from '$lib/features/wiki/WikiScreen.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { nameWindow } from '$lib/layout/window-title.svelte.ts';
  import { holdSaveKey } from '$lib/editor/save-key.ts';
  import { firstPage, withDescendants } from '$lib/logic/page-tree.ts';
  import {
    embeddedBoards,
    pageTrash,
    referenceTitles,
    whileHere,
    WikiStore,
    type PageStore,
    type Trash,
    type WikiLooks,
    type WikiPage,
    type Workspace,
    workspaceFavorites,
  } from '$lib/state/index.ts';

  import { Button } from '$lib/ui/button/index.js';

  import ManagedContainer from './ManagedContainer.svelte';
  import NotFound from './NotFound.svelte';
  import { referencePlaces } from './reference-places.ts';
  import type { Router } from './router.svelte.ts';

  let { workspace, router }: { workspace: Workspace; router: Router } = $props();

  // Where the references in the bodies of this workspace lead, by the app's one router:
  // the board page and the wiki page each say so, and neither takes it back.
  $effect.pre(() => {
    referenceTitles(workspace).places = referencePlaces(router);
  });

  const wikiId = $derived(router.route.kind === 'wiki' ? router.route.wikiId : undefined);
  const pageId = $derived(router.route.kind === 'wiki' ? router.route.pageId : undefined);

  /** The wiki of the address, made anew for another one. */
  const wiki = $derived(wikiId ? new WikiStore(workspace, wikiId) : undefined);
  nameWindow(() => {
    const shownPage = wiki?.openPage && wiki.page(wiki.openPage.pageId);
    return [
      shownPage && (shownPage.title || m.wiki_page_untitled()),
      wiki?.wiki && (wiki.wiki.title || m.wiki_untitled()),
    ];
  });
  // Read while it is shown, and closed when it goes. The board shown before lets go.
  $effect(() => {
    const store = wiki;
    if (!store) return;
    let deactivate: (() => void) | undefined;
    untrack(() => {
      deactivate = workspace.activateContainer(store);
      notice = undefined;
      joined = false;
      [trash, trashOpen, sharing] = [undefined, false, false];
      [versions, versionsOpen] = [undefined, false];
      const explicit = pageId;
      const remembered = explicit ?? workspace.rememberedWikiPage(store.wikiId);
      void store.load(
        remembered ? { pageId: remembered, speculative: explicit === undefined } : undefined,
      );
    });
    return () => {
      store.close();
      deactivate?.();
    };
  });

  // Without a page in the address: the remembered page once confirmed, else the first page.
  $effect(() => {
    if (wiki?.status !== 'ready' || pageId) return;
    const target = wiki.speculativePageId;
    if (target) {
      const open = wiki.openPage;
      if ((open?.pageId === target && open.status === 'ready') || wiki.page(target)) {
        router.openPage(target, { replace: true });
      }
      return;
    }
    if (!wiki.pagesReady) return;
    const first = firstPage(wiki.pages);
    if (first) router.openPage(first.id, { replace: true });
  });

  /** The reader joined the wiki here: the wiki's screen takes the focus, where the button was. */
  let joined = $state(false);

  /** What the head of the page tells: a page shown that somebody else deleted. */
  let notice = $state<string | undefined>(undefined);

  // The page shown is gone: to the first page that is left (none: the empty wiki), told if not the reader's doing.
  $effect(() => {
    const removed = wiki?.openPage?.removed;
    if (!wiki || !removed) return;
    if (removed.by === 'others') {
      notice = m.wiki_page_gone({ title: removed.title || m.wiki_page_untitled() });
    }
    router.openPage(firstPage(wiki.pages)?.id, { replace: true });
  });

  // The page the reader is on, told with the visit (before it starts: it is told once).
  $effect.pre(() => {
    wiki?.presence.setCard(pageId);
  });

  /**
   * The page the wiki is on is known: the address names one, or the wiki has
   * none to go to (the address of the wiki alone is replaced by its first page).
   */
  const placed = $derived(
    pageId !== undefined || !wiki || (wiki.pagesReady && firstPage(wiki.pages) === undefined),
  );

  // Who is on the wiki, while it is shown (joined or not); and on which page. The visit
  // is told once the page is known: not as the wiki alone, and then again with its page.
  $effect(() => {
    const shown = wiki;
    if (shown?.status === 'ready' && !shown.missing && placed) {
      return untrack(() => {
        shown.presence.setCard(pageId);
        return shown.presence.start();
      });
    }
    return undefined;
  });

  /** The page the reader asked to delete, while it is asked whether to. */
  let deleting = $state.raw<WikiPage | undefined>(undefined);
  let deleteOpen = $state(false);
  /** How many pages are below the page to delete: they go with it. */
  const below = $derived(deleting && wiki ? withDescendants(wiki.pages, deleting.id).size - 1 : 0);
  let settingsOpen = $state(false);
  let sharing = $state(false);
  let deleteWikiOpen = $state(false);
  /** The versions of a page, read anew each time their dialog opens; it goes with the page. */
  let versions = $state.raw<{ pageId: string; reading: VersionsReading }>();
  let versionsOpen = $state(false);
  /** The wiki's deleted pages, read anew each time they are opened. */
  let trash = $state.raw<Trash | undefined>(undefined);
  let trashOpen = $state(false);

  /** Deletes the wiki; the page goes on to the first board or wiki of the sidebar. */
  async function deleteWiki(store: WikiStore): Promise<void> {
    const done = await whileHere(workspace, async () => {
      await workspace.deleteContainer(store.wikiId);
      return true;
    });
    if (!done || workspace.containers.some((left) => left.id === store.wikiId)) return;
    // The reader went elsewhere meanwhile: they stay there.
    if (router.route.kind !== 'wiki' || router.route.wikiId !== store.wikiId) return;
    const next = workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (next) router.openContainer(next);
  }

  // The page of the address is the one open; none without one unless a speculative first page is loading.
  $effect(() => {
    const shown = wiki;
    const id = pageId;
    if (!shown) return;
    const status = shown.status;
    if (id !== undefined) {
      if (status === 'loading' || status === 'ready') untrack(() => shown.showPage(id));
    } else if (status === 'ready' && !shown.speculativePageId) {
      untrack(() => shown.showPage(undefined));
    }
  });
</script>

<!-- A page shown, the keys that keep a version of its body save no page anywhere on it. -->
<svelte:window
  onkeydown={(event: KeyboardEvent) => {
    if (wiki?.openPage) holdSaveKey(event);
  }}
  onpagehide={() => {
    void wiki?.settleBody();
  }}
/>

{#if !wiki || wiki.status === 'idle' || wiki.status === 'loading'}
  <div class="grid h-full place-items-center p-6">
    <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
  </div>
{:else if wiki.missing}
  <!-- Not there, or there to be managed only: ManagedContainer tells which. -->
  {@const store = wiki}
  <ManagedContainer
    {workspace}
    {store}
    kind="wiki"
    shown={() => workspace.activeContainer === store}
    {router}
    onJoined={() => {
      joined = true;
    }}
  />
{:else if wiki.status === 'failed'}
  <div class="grid h-full place-items-center p-6 text-center">
    <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
  </div>
{:else}
  {@const store = wiki}
  <div class="flex h-full min-h-0 flex-col">
    {#if wiki.joinable && !wiki.joinsOnWrite}
      <WikiJoin
        joining={wiki.joining}
        role={wiki.wiki?.minimumRole}
        onJoin={() => {
          void store.join().then((done) => {
            joined = done;
          });
        }}
      />
    {/if}
    <div class="min-h-0 flex-1">
      <WikiScreen
        {wiki}
        page={wiki.openPage}
        {notice}
        takeFocus={joined}
        onDismissNotice={() => {
          notice = undefined;
        }}
        pageHref={(id: string) => router.href({ ...router.route, pageId: id })}
        onDelete={(page: WikiPage) => {
          deleting = page;
          deleteOpen = true;
        }}
        onEditWiki={() => {
          settingsOpen = true;
        }}
        onDeleteWiki={() => {
          deleteWikiOpen = true;
        }}
        onShare={() => {
          sharing = true;
        }}
        onOpenTrash={() => {
          trash = pageTrash(wiki);
          trashOpen = true;
          void trash.read();
        }}
        onOpenPage={(id: string) => {
          router.openPage(id);
        }}
      >
        {#snippet missingPage()}
          <NotFound {router} />
        {/snippet}
        {#snippet body(page: PageStore, outlined: Attachment<HTMLElement>)}
          <!-- The page's body is edited together as a card's is, in the page's own room; its headings are the outline. -->
          <div {@attach outlined}>
            <CardBody
              container={wiki}
              card={page}
              of="page"
              testId="page-body"
              references={bodyReferences(wiki, page.pageId)}
              boardViews={embeddedBoards(workspace).host}
              keep={() => keepVersion(pageVersions(page))}
            />
          </div>
          {#if page.status === 'ready'}
            <CommentReactions reactions={page.comments} blockId={page.pageId} />
          {/if}
          <!-- WikiScreen draws each page anew: the list is read for the page it is drawn for. -->
          <CardBacklinks {workspace} blockId={page.pageId} of="page" heading="h2" />
          {@const comments = page.comments}
          {#if page.status === 'ready' && comments.status === 'ready'}
            <CardComments
              board={comments}
              card={comments}
              reactions={comments}
              readVersions={(commentId: string) => comments.commentVersions(commentId)}
              heading="h2"
            />
            {#if comments.canComment}
              <CommentComposer
                board={comments}
                card={comments}
                draftKey={page.pageId}
                placement="inline"
              />
            {/if}
          {:else if comments.status === 'failed'}
            <div class="flex flex-wrap items-center gap-2 border-t pt-5" role="alert">
              <p class="text-muted-foreground">{m.app_problem_load()}</p>
              <Button variant="outline" onclick={() => void comments.reread()}>
                {m.app_retry()}
              </Button>
            </div>
          {/if}
        {/snippet}
        {#snippet pageActions(page: PageStore)}
          {@const shown = wiki.page(page.pageId) ?? page.meta}
          {#if shown}
            <WatchToggle {workspace} blockId={page.pageId} kind="page" />
            {#if !wiki.container?.isTemplate}
              <FavoriteToggle stars={workspaceFavorites(workspace)} targetId={page.pageId} />
            {/if}
            <PageMenu
              {workspace}
              page={shown}
              href={router.href({ ...router.route, pageId: page.pageId })}
              onVersions={() => {
                versions = {
                  pageId: page.pageId,
                  reading: new VersionsReading(pageVersions(page)),
                };
                versionsOpen = true;
              }}
            />
          {/if}
        {/snippet}
      </WikiScreen>
    </div>
  </div>
  <!-- Another page shown, the dialog of the one before goes with it. -->
  {#if versions && versions.pageId === pageId}
    <BodyVersions bind:open={versionsOpen} reading={versions.reading} container={wiki} of="page" />
  {/if}
  {@const title = deleting?.title || m.wiki_page_untitled()}
  <ConfirmDialog
    bind:open={deleteOpen}
    title={m.wiki_delete_page_title()}
    description={below > 0
      ? m.wiki_delete_page_below({ title, count: below })
      : m.wiki_delete_page_alone({ title })}
    confirmLabel={m.shared_delete()}
    destructive
    onConfirm={() => {
      if (deleting) void store.deletePage(deleting.id);
    }}
    focusAfter={() =>
      // The page's row is gone with it: the tree's place in the tab order instead.
      document.querySelector<HTMLElement>(
        '[data-testid="page-tree"] [role="treeitem"][tabindex="0"]',
      )}
  />
  {#if store.canEdit}
    <TrashDialog
      bind:open={trashOpen}
      {trash}
      {workspace}
      title={m.wiki_trash()}
      description={m.wiki_trash_retention({ days: workspace.trashRetentionDays })}
      untitled={m.wiki_page_untitled()}
      testId="page-trash"
    />
  {/if}
  {#if store.wiki}
    <!-- Whoever reads the wiki sees its members; only its admins change them or share it. -->
    <ShareDialog
      bind:open={sharing}
      {workspace}
      shareable={store}
      membersOnly={!store.canManage}
      readOnly={!store.canManage}
      linkOf={(readToken: string) =>
        new URL(router.href({ boardId: store.wikiId, readToken }), location.href).href}
    />
    <WikiSettingsDialog
      bind:open={settingsOpen}
      wiki={store.wiki}
      onSave={(looks: WikiLooks) => void store.changeWiki(looks)}
    />
  {/if}
  <ConfirmDialog
    bind:open={deleteWikiOpen}
    title={m.shell_delete_wiki_title()}
    description={m.shell_delete_wiki_info({ title: store.wiki?.title || m.wiki_untitled() })}
    confirmLabel={m['DeleteBoardDialog.confirm-delete']()}
    destructive
    onConfirm={() => void deleteWiki(store)}
  />
{/if}
