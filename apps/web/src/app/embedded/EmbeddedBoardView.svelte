<!--
  A view the reader sees, shown in a body: a header naming the board and the
  view, with the kind of view and a link to open it on the board; then the
  view's cards as the board shows them (its filter and sort, the properties
  it shows, its grouping), at most `EMBEDDED_CARDS`, and a link to all of
  them on the board.

  Drawn from the board's store held for the views shown in bodies
  (`embeddedBoards`), which follows the board's changes: a card changed on
  the board, by this reader or another, is changed here too. Who may change
  cards changes a checkbox and the status here (EmbeddedValue.svelte).

  A card pressed opens over the page: by the address where the page is its
  own board, otherwise in the card's dialog the host keeps over whatever page
  this is (`onOpenCard`, BoardViewHost.svelte).
-->
<script lang="ts">
  import { onDestroy, untrack } from 'svelte';

  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import { followedHere } from '$lib/components/menu/actions.ts';
  import { tableColumns } from '$lib/features/views/table/columns.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { dateOfInstant } from '$lib/logic/dates.ts';
  import type { BoardView } from '$lib/logic/wire.ts';
  import { embeddedBoards, type LinkedBlock, type Workspace } from '$lib/state/index.ts';

  import type { RouteTarget } from '../route-path.ts';
  import type { Router } from '../router.svelte.ts';
  import EmbeddedColumns from './EmbeddedColumns.svelte';
  import EmbeddedDays from './EmbeddedDays.svelte';
  import EmbeddedRows from './EmbeddedRows.svelte';
  import {
    EMBEDDED_CARDS,
    firstCards,
    kindName,
    summaryOf,
    type EmbeddedHost,
  } from './embedded-view.ts';

  interface Props {
    workspace: Workspace;
    router: Router;
    /** The view as the reader is told of it. Its board does not change: another is another of this. */
    linked: LinkedBlock;
    /** Opens a card of the board over a page that is not the board's. */
    onOpenCard: (boardId: string, view: BoardView, cardId: string) => void;
  }

  let { workspace, router, linked, onOpenCard }: Props = $props();

  const held = untrack(() => embeddedBoards(workspace).hold(linked.containerId));
  const store = held.store;
  onDestroy(held.release);

  const presentation = new PickerPresentation();

  const view = $derived(store.view(linked.id));
  const filtered = $derived(view ? store.filteredCardsOf(view) : []);
  const shown = $derived(firstCards(filtered));
  const boardTitle = $derived(
    store.board?.title || linked.containerTitle || m['Sidebar.untitled-board'](),
  );
  const viewTitle = $derived(view?.title || m['Sidebar.untitled-view']());
  const workspaceId = $derived(workspace.info?.id ?? '');

  /** Where the view is on its board. */
  const viewRoute = $derived<RouteTarget>({
    workspaceId,
    boardId: store.boardId,
    viewId: linked.id,
  });

  const host = $derived.by<EmbeddedHost | undefined>(() => {
    if (!view) return undefined;
    return {
      store,
      view,
      canEdit: store.canEdit,
      locale: workspace.locale,
      regional: workspace.regional,
      users: workspace.users,
      people: store.people,
      presentation: presentation.current,
      columns: tableColumns(view, store.board?.cardProperties ?? []),
      cardHref: (cardId) => router.href({ ...viewRoute, cardId }),
      open: (cardId) => {
        const route = router.route;
        if (route.kind === 'board' && route.boardId === store.boardId) router.openCard(cardId);
        else onOpenCard(store.boardId, view, cardId);
      },
    };
  });

  const today = $derived(dateOfInstant(Date.now(), workspace.regional.timeZone));

  /** Follows a link to the board where the page answers a plain press itself. */
  function go(event: MouseEvent, route: RouteTarget): void {
    if (!followedHere(event)) return;
    event.preventDefault();
    router.go(route);
  }
</script>

{#if store.missing || (store.status === 'ready' && !view)}
  <p class="doc-board-view-unseen" data-testid="board-view-unseen">{m.board_view_unseen()}</p>
{:else if store.status === 'failed'}
  <p class="embedded-view-said" role="alert">{m.board_view_failed()}</p>
{:else if !host || !view}
  <p class="embedded-view-said" role="status">{m.board_view_loading()}</p>
{:else}
  <div
    class="embedded-view-frame"
    role="region"
    aria-label={m.board_view_region({ board: boardTitle, view: viewTitle })}
    data-testid="embedded-view"
  >
    <header class="embedded-view-header">
      <div class="min-w-0 flex-1">
        <p class="truncate text-xs text-muted-foreground">{boardTitle}</p>
        <p class="flex min-w-0 items-center gap-2">
          <span class="truncate font-medium">{viewTitle}</span>
          <span class="embedded-view-kind" data-testid="embedded-view-kind">
            {kindName(view.fields.viewType)}
          </span>
        </p>
      </div>
      <a
        class="embedded-view-open"
        href={router.href(viewRoute)}
        onclick={(event: MouseEvent) => {
          go(event, viewRoute);
        }}
      >
        {m.board_view_open()}
      </a>
    </header>
    {#if filtered.length === 0}
      <p class="embedded-view-said">{m.board_view_no_cards()}</p>
    {:else}
      <div class="embedded-view-scroll" data-testid="embedded-view-scroll">
        {#if summaryOf(view.fields.viewType) === 'columns'}
          <EmbeddedColumns {host} cards={shown} />
        {:else if summaryOf(view.fields.viewType) === 'days'}
          <!-- A calendar picks its fifty from all: the days still to come may be past the first fifty. -->
          <EmbeddedDays {host} cards={filtered} {today} />
        {:else}
          <EmbeddedRows {host} cards={shown} />
        {/if}
      </div>
    {/if}
    {#if filtered.length > EMBEDDED_CARDS}
      <a
        class="embedded-view-all"
        href={router.href(viewRoute)}
        data-testid="embedded-view-all"
        onclick={(event: MouseEvent) => {
          go(event, viewRoute);
        }}
      >
        {m.board_view_see_all({ count: filtered.length })}
      </a>
    {/if}
  </div>
{/if}
