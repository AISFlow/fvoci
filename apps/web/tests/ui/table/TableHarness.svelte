<!-- The table over a board store, the way the board page holds it: the view and its cards follow the store. -->
<script lang="ts">
  import Table from '$lib/features/views/table/Table.svelte';
  import type { BoardStore } from '$lib/state/index.ts';

  interface Props {
    board: BoardStore;
    viewId: string;
    onOpenCard?: (cardId: string) => void;
  }

  let { board, viewId, onOpenCard = () => undefined }: Props = $props();

  const view = $derived(board.view(viewId));
  const cards = $derived(view ? board.cardsOf(view) : []);
</script>

{#if view}
  <Table {board} {view} {cards} cardHref={(cardId: string) => `/card/${cardId}`} {onOpenCard} />
{/if}
