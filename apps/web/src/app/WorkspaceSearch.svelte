<!--
  The search of the workspace's cards and pages, where the features meet:
  its button (in the top bar), its keys (⌘K on Apple's devices, Ctrl+K elsewhere), its dialog, and going
  to what it found by the router.

  Only where the server says it searches bodies (`searchEnabled`, which
  every server does: on its search server, or on its database): until that
  is known there is no button and the keys are the browser's. A search
  server that does not answer, or cannot hold its index, answers each search
  503, which the dialog says.

  `#12` finds `#12` cards across every readable board in the workspace, and
  where a board is shown, that board's card (`#12`, `12`) comes first
  (`numbered-search.ts`). A card opens over the view of its board that is
  shown, or else over the board's first view; a page at its wiki's address
  (`places.ts`). The name of the board or wiki of a hit is the one the
  sidebar knows, or else the title the search returned.

  Above what was found, the reader's stars (`WorkspaceFavorites`) and, before
  anything is typed, what they opened last on this browser
  (`recent-places.ts`, at most eight, never of a diary); then the boards,
  wikis, diaries and courses to go to at once (`SearchJumps`): the first of
  the sidebar's before anything is typed, then those whose title holds it.
  In the reader's own space the entries of diaries are a kind to find, each
  named by its day.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import SearchIcon from '@lucide/svelte/icons/search';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import {
    opensSearch,
    searchKeysLabel,
    searchKeysNames,
  } from '$lib/features/search/search-keys.ts';
  import { findNumbered, type NumberedSearch } from '$lib/features/search/numbered-search.ts';
  import { SearchSession } from '$lib/features/search/search-session.svelte.ts';
  import { SearchSwitch } from '$lib/features/search/search-switch.svelte.ts';
  import SearchDialog from '$lib/features/search/SearchDialog.svelte';
  import { favoriteTitle, shownTitle } from '$lib/features/shell/shown-title.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { overlayOpen } from '$lib/layout/back.svelte.ts';
  import { Opener } from '$lib/focus.ts';
  import { favoriteJumps, searchJumps } from '$lib/logic/search-jumps.ts';
  import type { Container } from '$lib/logic/wire.ts';
  import {
    isOwnSpace,
    workspaceFavorites,
    type Favorite,
    type Workspace,
  } from '$lib/state/index.ts';
  import type { SearchHit } from '$lib/state/content-search.ts';

  import { focusThePlace, goToPlace, placeOfFavorite, routeOfPlace, type Place } from './places.ts';
  import { shownRecents } from './recent-places.ts';
  import { placeRoute } from './route-path.ts';
  import type { Router } from './router.svelte.ts';
  import SearchJumps from './SearchJumps.svelte';
  import SearchPlaces from './SearchPlaces.svelte';

  let { workspace, router }: { workspace: Workspace; router: Router } = $props();

  let open = $state(false);
  let session = $state.raw<SearchSession | undefined>(undefined);
  let dialog = $state<{ focusField: () => void; leaving: () => void }>();
  const opener = new Opener();

  /** Asked of each backend the workspace has (one, for the life of the page). */
  const enabled = $derived(new SearchSwitch(workspace.backend.search));

  const keys = searchKeysLabel();

  const favorites = $derived(workspaceFavorites(workspace));
  /** What the reader opened last here, read each time the search opens. */
  let recents = $state.raw<readonly Favorite[]>([]);
  /** Which opening of the search the places read last are of: an earlier one's are not shown. */
  let recentsTurn = 0;

  /** Where the focus goes back to: where it was, or the page where that is gone. */
  function returnTo(): HTMLElement | undefined {
    return opener.take() ?? document.querySelector('main') ?? undefined;
  }

  /**
   * Opens the search, from `from` (the button pressed); whether it did. Never over another overlay: the wiki's drawer of
   * pages leaves the top bar out of its reach, and going from the search to a
   * page of the same wiki would leave it open, without its entry of the history.
   */
  function openSearch(from?: HTMLElement): boolean {
    const workspaceId = workspace.info?.id;
    if (!enabled.on || !workspaceId || otherOverlayOpen()) return false;
    const focused = document.activeElement;
    // Not the field of the search that was just closed and is fading out: it goes.
    const kept = focused instanceof HTMLElement && !focused.closest('[data-search-dialog]');
    opener.hold(from ?? (kept ? focused : undefined));
    session?.close();
    const { backend } = workspace;
    const search: NumberedSearch = {
      find: (ask) => backend.search.find(workspaceId, ask),
      numbered: (boardId, number) => backend.references.numbered(workspaceId, boardId, number),
      boardShown: () => (router.route.kind === 'board' ? router.route.boardId : undefined),
    };
    session = new SearchSession({
      find: (ask) => findNumbered(search, ask),
      onRefused: (error) => {
        workspace.report('load', error);
      },
    });
    open = true;
    void favorites.read();
    recents = [];
    const turn = ++recentsTurn;
    void shownRecents(workspace).then((shown) => {
      // Not those of an opening before, nor of the workspace left meanwhile.
      if (open && turn === recentsTurn && workspace.info?.id === workspaceId) recents = shown;
    });
    return true;
  }

  /**
   * Whether a drawer, a sheet or a dialog other than a card's is open: all are
   * modal (`aria-modal`), and those the back button closes say so too. A
   * dialog closed and still fading out (the search's own, after a hit was
   * chosen) is none.
   */
  function otherOverlayOpen(): boolean {
    const other = '[aria-modal="true"]:not([data-card-dialog], [data-state="closed"])';
    return overlayOpen() || document.querySelector(other) !== null;
  }

  /**
   * ⌘K, or Ctrl+K off Apple's devices. Open, they go back to the field (held down, they do
   * nothing more). Over a drawer, a sheet or another dialog they open
   * nothing: going somewhere from the search would leave that open over the
   * new page, and its entry of the history with it. A card is no such thing:
   * the search opens over it.
   */
  function onkeydown(event: KeyboardEvent): void {
    if (!enabled.on || !opensSearch(event)) return;
    if (open) {
      event.preventDefault();
      if (!event.repeat) dialog?.focusField();
      return;
    }
    if (!event.repeat && openSearch()) event.preventDefault();
  }

  /** A hit, in the workspace shown. */
  function placeOf(hit: SearchHit): Place {
    return { ...hit, workspaceId: workspace.info?.id };
  }

  function hrefOf(hit: SearchHit): string {
    return router.href(routeOfPlace(router, placeOf(hit)));
  }

  /**
   * Closes the search, then goes to a hit (`goToPlace`), once the dialog has
   * given its entry of the history back: the history is then as if the hit
   * was opened from the page.
   */
  async function go(hit: SearchHit): Promise<void> {
    const place = placeOf(hit);
    session?.close();
    open = false;
    await tick();
    goToPlace(router, place);
    await tick();
    settleFocus();
  }

  /**
   * After going: the focus, still in the field fading out (and then on
   * nothing), goes to the card shown, else to the page. A card that opens
   * later, over another board, takes it from there.
   */
  function settleFocus(): void {
    const at = document.activeElement;
    if (at instanceof HTMLElement && at !== document.body && !at.closest('[data-search-dialog]')) {
      return;
    }
    focusThePlace();
  }

  /** Closes the search, then opens a board, a wiki, a diary or a course, as from the sidebar. */
  async function jump(container: Container): Promise<void> {
    dialog?.leaving();
    session?.close();
    open = false;
    await tick();
    router.openContainer(container);
    await tick();
    settleFocus();
  }

  function favoriteHref(place: Favorite): string {
    return router.href(routeOfPlace(router, placeOfFavorite(place, workspace.info?.id)));
  }

  /** Closes the search, then goes to a star or to what was opened last, as to a hit. */
  async function goToFavorite(place: Favorite): Promise<void> {
    dialog?.leaving();
    session?.close();
    open = false;
    await tick();
    goToPlace(router, placeOfFavorite(place, workspace.info?.id));
    await tick();
    settleFocus();
  }

  function containerOf(hit: SearchHit): string {
    const container = workspace.containers.find((one) => one.id === hit.containerId);
    if (container) return shownTitle(container);
    if (hit.containerTitle !== undefined) {
      return shownTitle({ title: hit.containerTitle, kind: hit.containerKind });
    }
    return '';
  }
</script>

<svelte:window {onkeydown} />

{#snippet jumps(query: string)}
  {@const starred = favoriteJumps(favorites.list, query, favoriteTitle)}
  {@const found = searchJumps(workspace.containers, query, shownTitle)}
  {#if starred.length > 0}
    <SearchPlaces
      heading={m.favorites_search()}
      testId="search-favorites"
      places={starred}
      locale={workspace.locale}
      hrefOf={favoriteHref}
      onGo={(place: Favorite) => void goToFavorite(place)}
    />
  {/if}
  {#if query.trim() === '' && recents.length > 0}
    <SearchPlaces
      heading={m.favorites_recent()}
      testId="search-recent"
      places={recents}
      locale={workspace.locale}
      hrefOf={favoriteHref}
      onGo={(place: Favorite) => void goToFavorite(place)}
    />
  {/if}
  {#if found.length > 0}
    <SearchJumps
      containers={found}
      locale={workspace.locale}
      hrefOf={(container: Container) => router.href(placeRoute(container))}
      onJump={(container: Container) => void jump(container)}
    />
  {/if}
{/snippet}

{#if enabled.on}
  <IconButton
    label={m.search_open()}
    title={m.search_open_keys({ keys })}
    data-testid="open-search"
    aria-haspopup="dialog"
    aria-keyshortcuts={searchKeysNames()}
    onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) => {
      openSearch(event.currentTarget);
    }}
  >
    <SearchIcon />
  </IconButton>
{/if}

{#if session}
  <SearchDialog
    bind:open={
      () => open,
      (next: boolean) => {
        open = next;
        if (!next) session?.close();
      }
    }
    {session}
    {containerOf}
    {hrefOf}
    bind:this={dialog}
    onGo={(hit: SearchHit) => void go(hit)}
    {returnTo}
    withEntries={isOwnSpace(workspace.info)}
    locale={workspace.locale}
    {jumps}
  />
{/if}
