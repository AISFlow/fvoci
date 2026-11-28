<!--
  What draws and puts in the board views of the workspace's bodies
  (`embeddedBoards(workspace).host`, read by the bodies of cards, pages and
  units; board-view.ts): set while the workspace's pane is shown.

  - `mount` draws EmbeddedView.svelte into a board view's place, with the
    contexts of this pane (a component mounted by hand has none of its own);
  - `choose` opens the window that asks for a board and a view
    (BoardViewPicker.svelte), one at a time: asking again gives up the
    asking before;
  - `viewAt` reads the address of a view of the workspace entered;
  - `seen` asks whether the reader sees a view, as a reference's title is.

  A card pressed in a board view on a page that is not its board's opens
  here, in one card's dialog over the page, from the store of the board's
  views, which is held while the card is open.
-->
<script lang="ts">
  import { getAllContexts, mount, onDestroy, unmount } from 'svelte';

  import type { BoardViewHost } from '$lib/editor/board-view-read.ts';
  import CardDialog from '$lib/features/card/CardDialog.svelte';
  import { boardViewAt } from '$lib/logic/reference-address.ts';
  import type { BoardView } from '$lib/logic/wire.ts';
  import {
    embeddedBoards,
    referenceTitles,
    type HeldBoard,
    type Workspace,
  } from '$lib/state/index.ts';

  import { workspaceIdOf, type Router } from '../router.svelte.ts';
  import BoardViewPicker from './BoardViewPicker.svelte';
  import EmbeddedView from './EmbeddedView.svelte';

  interface Props {
    workspace: Workspace;
    router: Router;
  }

  let { workspace, router }: Props = $props();

  const context = getAllContexts();

  /** The asking now: what is told the view chosen, or none. */
  let asking = $state.raw<((viewId: string | undefined) => void) | undefined>(undefined);

  /** The board of the card open over the page, held while it is, and the view it was opened from. */
  let opened = $state.raw<{ held: HeldBoard; view: BoardView } | undefined>(undefined);
  let openedCard = $state<string | undefined>(undefined);

  function openCard(boardId: string, view: BoardView, cardId: string): void {
    if (opened?.held.store.boardId === boardId) {
      opened = { ...opened, view };
    } else {
      letCardGo();
      opened = { held: embeddedBoards(workspace).hold(boardId), view };
    }
    openedCard = cardId;
  }

  /** Closes the card: what was written in its body is sent as it leaves (`closeCard`). */
  function letCardGo(): void {
    const was = opened;
    [opened, openedCard] = [undefined, undefined];
    was?.held.store.closeCard();
    was?.held.release();
  }

  onDestroy(letCardGo);

  /** Tells the asking what was chosen, and ends it. */
  function answer(viewId: string | undefined): void {
    const tell = asking;
    asking = undefined;
    tell?.(viewId);
  }

  function hostOf(shown: Workspace): BoardViewHost {
    return {
      mount: (place, viewId) => {
        const drawn = mount(EmbeddedView, {
          target: place,
          props: { workspace: shown, router, viewId, onOpenCard: openCard },
          context,
        });
        return () => {
          void unmount(drawn);
        };
      },
      choose: () =>
        new Promise<string | undefined>((resolve) => {
          answer(undefined);
          asking = resolve;
        }),
      viewAt: (address) =>
        boardViewAt(address, {
          origin: location.origin,
          basePath: router.basePath,
          workspaceId: workspaceIdOf(router.route),
        }),
      seen: async (viewId) => {
        const titles = referenceTitles(shown);
        return (await titles.seen(viewId)) && titles.of(viewId)?.kind === 'view';
      },
    };
  }

  $effect.pre(() => {
    const boards = embeddedBoards(workspace);
    const host = hostOf(workspace);
    boards.host = host;
    return () => {
      if (boards.host === host) boards.host = undefined;
      answer(undefined);
    };
  });
</script>

<BoardViewPicker
  bind:open={
    () => asking !== undefined,
    (open: boolean) => {
      if (!open) answer(undefined);
    }
  }
  {workspace}
  onPick={(viewId: string) => {
    answer(viewId);
  }}
/>

{#if opened}
  {@const { store } = opened.held}
  {@const viewId = opened.view.id}
  <CardDialog
    board={store}
    view={opened.view}
    cardId={openedCard}
    cardHref={(cardId: string) =>
      router.href({
        workspaceId: workspaceIdOf(router.route),
        boardId: store.boardId,
        viewId,
        cardId,
      })}
    onOpenCard={(cardId: string) => {
      openedCard = cardId;
    }}
    onClose={letCardGo}
  />
{/if}
