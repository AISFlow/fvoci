<!--
  What the board page does for the header, for a test: it reads the view
  from the store, so that the header gets the view anew after every change,
  and it holds the search text.
-->
<script lang="ts">
  import ViewHeader from '$lib/features/view-header/ViewHeader.svelte';
  import type { BoardStore } from '$lib/state/index.ts';

  interface Props {
    board: BoardStore;
    viewId: string;
    onOpenView?: (viewId: string) => void;
    onAddCard?: () => void;
  }

  let {
    board,
    viewId,
    onOpenView = () => undefined,
    onAddCard = () => undefined,
  }: Props = $props();

  const view = $derived(board.view(viewId));
  let search = $state('');
</script>

{#if view}
  <div data-testid="board-page">
    <ViewHeader
      {board}
      {view}
      bind:search
      viewHref={(id: string) => `/view/${id}`}
      {onOpenView}
      {onAddCard}
    />
  </div>
{/if}
<output data-testid="search-text">{search}</output>
