<!--
  One board: the header of its view, the view itself, and the card that is
  open over it. Which board, view and card comes from the address; this
  component keeps the store and the address in step.

  The parts are features of their own and meet here (src/lib/README.md):
  `view-header`, the views (`kanban`, `table`, `gallery`, `calendar`, `gantt`), and `card`.
-->
<script lang="ts" module>
  import { LazyChunk } from '$lib/chunk-reload.svelte.ts';

  const tableChunk = new LazyChunk(() => import('$lib/features/views/table/Table.svelte'));
  const galleryChunk = new LazyChunk(() => import('$lib/features/views/gallery/Gallery.svelte'));
  const ganttChunk = new LazyChunk(() => import('$lib/features/views/gantt/Gantt.svelte'));
  const calendarChunk = new LazyChunk(() => import('$lib/features/views/calendar/Calendar.svelte'));
</script>

<script lang="ts">
  import { tick, untrack } from 'svelte';

  import CardDialog from '$lib/features/card/CardDialog.svelte';
  import ViewHeader from '$lib/features/view-header/ViewHeader.svelte';
  import Kanban from '$lib/features/views/kanban/Kanban.svelte';
  import WikiJoin from '$lib/features/wiki/WikiJoin.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { nameWindow } from '$lib/layout/window-title.svelte.ts';
  import { focusAdrift } from '$lib/focus.ts';
  import { propertiesThatMeetFilterGroup } from '$lib/logic/filter.ts';
  import {
    boardPresence,
    newCardMaker,
    referenceTitles,
    whileHere,
    type CardStore,
    type Workspace,
  } from '$lib/state/index.ts';

  import LazyView from './LazyView.svelte';
  import ManagedContainer from './ManagedContainer.svelte';
  import { referencePlaces } from './reference-places.ts';
  import { cardIdOf, isSharedRoute, workspaceIdOf, type Router } from './router.svelte.ts';
  import TimeTargetControl from './TimeTargetControl.svelte';

  let { workspace, router }: { workspace: Workspace; router: Router } = $props();

  $effect.pre(() => {
    referenceTitles(workspace).places = referencePlaces(router);
  });

  const routeBoardId = $derived(
    router.route.kind === 'board' || router.route.kind === 'shared'
      ? router.route.boardId
      : undefined,
  );
  const routeViewId = $derived(
    router.route.kind === 'board' || router.route.kind === 'shared'
      ? router.route.viewId
      : undefined,
  );
  const routeCardId = $derived(cardIdOf(router.route));
  const routeWorkspaceId = $derived(workspaceIdOf(router.route));
  const routeReadToken = $derived(
    router.route.kind === 'shared' ? router.route.readToken : undefined,
  );

  // Open the board the address names.
  $effect(() => {
    const boardId = routeBoardId;
    if (boardId && workspace.board?.boardId !== boardId) void workspace.openBoard(boardId);
  });

  const board = $derived(workspace.board?.boardId === routeBoardId ? workspace.board : undefined);
  const view = $derived(board?.view(routeViewId));
  const shownCard = $derived(routeCardId ? board?.cardsById.get(routeCardId) : undefined);
  nameWindow(() => [
    shownCard && (shownCard.title || m['KanbanCard.untitled']()),
    board?.board && (board.board.title || m['Sidebar.untitled-board']()),
  ]);

  // The card the reader is on, before the visit starts: the visit is told once, with it.
  $effect(() => {
    if (board) boardPresence(board).setCard(routeCardId);
  });
  $effect(() => {
    const shown = board;
    if (shown?.status === 'ready' && !shown.missing) {
      return untrack(() => boardPresence(shown).start());
    }
    return undefined;
  });

  let page = $state<HTMLElement>();

  /** Joins the open board shown, then puts the focus on the board, where the button was. */
  async function join(): Promise<void> {
    if (!board || !(await board.join())) return;
    await tick();
    page?.focus();
  }

  /** The view shown last: not reactive, it tells a view that went from one never shown. */
  let shownId: string | undefined;
  $effect(() => {
    if (view) shownId = view.id;
  });

  // An address without a view, or with one that is gone, shows the first view. A view that
  // goes while it is shown (deleted from its own list, or by someone else) takes the page
  // with it, and the focus if it was there: that goes to the button of the views, which
  // names the view shown now.
  $effect(() => {
    if (board?.status !== 'ready' || view) return;
    if (routeViewId && board.revalidating) return;
    const first = board.orderedViews[0];
    if (!first) return;
    const went = shownId !== undefined && routeViewId === shownId;
    router.go({ ...router.route, viewId: first.id }, { replace: true });
    if (went) void tick().then(focusViews);
  });

  function focusViews(): void {
    const button = page?.querySelector<HTMLElement>('[data-tool="views"]');
    if (focusAdrift(button)) button?.focus();
  }

  /** What the reader typed into the search of the view. It is theirs alone and not stored. */
  let search = $state('');
  // Another view starts without a search. By its identifier: the view is a
  // new object after every change to it, and a change is not another view.
  const viewId = $derived(view?.id);
  $effect(() => {
    if (viewId) search = '';
  });

  const filteredCards = $derived(board && view ? board.filteredCardsOf(view) : []);
  const cards = $derived(board ? board.narrowCards(filteredCards, search) : []);

  const cardHref = (cardId: string): string =>
    router.href({
      workspaceId: routeWorkspaceId,
      boardId: routeBoardId,
      viewId: routeViewId,
      readToken: routeReadToken,
      cardId,
    });
  const openCard = (cardId: string): void => {
    router.openCard(cardId);
  };

  /**
   * A new card that the view will show: made the view's way where it has one
   * (a day on a calendar or a Gantt chart), else with the values the view's
   * filter asks for.
   */
  async function addCard(): Promise<void> {
    if (!board?.board || !view) return;
    const maker = newCardMaker(view.id);
    if (maker) {
      await maker();
      return;
    }
    const properties = propertiesThatMeetFilterGroup(
      view.fields.filter,
      board.board.cardProperties,
      workspace.me?.id,
    );
    const card = await whileHere(board, () => board.createCard({ properties }));
    if (card) openCard(card.id);
  }
</script>

<svelte:window
  onpagehide={() => {
    void board?.openCard?.settleBody();
  }}
/>

{#if !board || board.status === 'idle' || board.status === 'loading'}
  <div class="grid h-full place-items-center p-6">
    <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
  </div>
{:else if board.missing}
  <!-- Not there, or there to be managed only: ManagedContainer tells which. -->
  {@const store = board}
  <ManagedContainer
    {workspace}
    {store}
    shown={() => workspace.board === store}
    {router}
    onJoined={() => {
      void tick().then(() => page?.focus());
    }}
  />
{:else if board.status === 'failed'}
  <div class="grid h-full place-items-center p-6 text-center">
    <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
  </div>
{:else if view}
  <div
    bind:this={page}
    class="relative flex h-full min-h-0 flex-col outline-none"
    tabindex="-1"
    data-testid="board-page"
    style:--fab-room={board.canEdit && !pane.row ? '5rem' : undefined}
  >
    {#if board.joinable && !board.joinsOnWrite}
      <WikiJoin
        kind="board"
        joining={board.joining}
        role={board.board?.minimumRole}
        onJoin={() => void join()}
      />
    {/if}
    <ViewHeader
      {board}
      {view}
      bind:search
      viewHref={(viewId: string) => router.href({ ...router.route, viewId, cardId: undefined })}
      onOpenView={(viewId: string) => {
        router.openView(viewId);
      }}
      onAddCard={() => void addCard()}
      exportCards={isSharedRoute(router.route) ? undefined : () => cards}
      {cardHref}
    />
    <div class="min-h-0 flex-1">
      {#if view.fields.viewType === 'board'}
        <Kanban {board} {view} {cards} {cardHref} onOpenCard={openCard} />
      {:else if view.fields.viewType === 'table'}
        <LazyView chunk={tableChunk}>
          {#snippet children(Table)}
            <Table {board} {view} {cards} {cardHref} onOpenCard={openCard} />
          {/snippet}
        </LazyView>
      {:else if view.fields.viewType === 'gallery'}
        <LazyView chunk={galleryChunk}>
          {#snippet children(Gallery)}
            <Gallery {board} {view} {cards} {cardHref} onOpenCard={openCard} />
          {/snippet}
        </LazyView>
      {:else if view.fields.viewType === 'gantt'}
        <!-- Another Gantt view starts with today in sight: where it is scrolled to is not this one's. -->
        <LazyView chunk={ganttChunk}>
          {#snippet children(Gantt)}
            {#key view.id}
              <Gantt {board} {view} {cards} {cardHref} onOpenCard={openCard} />
            {/key}
          {/snippet}
        </LazyView>
      {:else}
        <!-- Another calendar view starts at today: its month and day are not this one's. -->
        <LazyView chunk={calendarChunk}>
          {#snippet children(Calendar)}
            {#key view.id}
              <Calendar {board} {view} {cards} {cardHref} onOpenCard={openCard} />
            {/key}
          {/snippet}
        </LazyView>
      {/if}
    </div>
  </div>
  <CardDialog
    {board}
    {view}
    {cardHref}
    cardId={routeCardId}
    onOpenCard={openCard}
    onClose={() => {
      router.closeCard();
    }}
  >
    {#snippet headerActions(card: CardStore)}
      {#if !isSharedRoute(router.route) && workspace.backend.time && workspace.me?.id && !board.board?.isTemplate && card.summary && !card.summary.fields.isTemplate && !card.missing}
        <TimeTargetControl
          {workspace}
          workspaceId={board.board?.workspaceId}
          containerId={board.boardId}
          blockId={card.cardId}
          defaultKind="work"
        />
      {/if}
    {/snippet}
  </CardDialog>
{/if}
