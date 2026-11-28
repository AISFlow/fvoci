<!--
  The quick filters of the board shown: chips under the header of the view
  that narrow its cards for the reader alone, whatever view they look at
  (lib/logic/quick-filter.ts). The view is not changed for anyone else, and
  nothing of them goes to the server; they are kept for the reader in this
  browser (lib/state/board-quick.svelte.ts).

  A chip is a button that is pressed or not. A board offers the chips it has
  what for: "Mine" to a reader who is signed in, "Not done" where it has a
  rule of done, and the two of the due date where it has one. While chips
  are on, the row ends in a button that turns them all off and says how many
  there are; the focus then goes to the first chip. In a narrow pane the row
  is one line that scrolls sideways.

  Once the row is shown on a visit to the board and its cards are in (not
  while they are loaded or read again), the cards there are noted: a card
  the reader makes after that stays shown whatever the chips are.
-->
<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';
  import { tick, untrack } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { offeredQuickFilters } from '$lib/logic/quick-filter.ts';
  import { quickFilters } from '$lib/state/board-quick.svelte.ts';
  import type { BoardStore } from '$lib/state/index.ts';

  import { QUICK_FILTER_NAMES } from './texts.ts';

  let { board }: { board: BoardStore } = $props();

  /** A public link's reader has an empty identifier: nobody is signed in. */
  const meId = $derived(board.workspace.me?.id || undefined);
  const offered = $derived(offeredQuickFilters(board.board, meId));
  /** The chips that are on, of those offered: one that is not shown does not narrow. */
  const on = $derived(
    quickFilters.chosen(board.boardId, meId).filter((id) => offered.includes(id)),
  );

  /** Turns every chip off, and gives the focus to the first: the button pressed is gone. */
  async function clearAll(row: HTMLElement | null): Promise<void> {
    quickFilters.clear(board, meId);
    await tick();
    row?.querySelector<HTMLElement>('[data-quick-filter]')?.focus();
  }
</script>

{#if offered.length > 0}
  <div
    role="group"
    aria-label={m.viewheader_quick_filters()}
    data-testid="quick-filters"
    {@attach () => {
      const shown = board;
      if (shown.status !== 'ready' || shown.revalidating) return;
      untrack(() => {
        quickFilters.opened(shown);
      });
    }}
    class={[
      'flex shrink-0 [scrollbar-width:none] items-center gap-1.5 overflow-x-auto overscroll-x-contain border-b py-1.5',
      pane.row ? 'px-4' : 'px-1.5',
    ]}
  >
    {#each offered as id (id)}
      <button
        type="button"
        aria-pressed={on.includes(id)}
        data-quick-filter={id}
        class="h-7 shrink-0 rounded-full border border-input-border px-3 text-sm whitespace-nowrap text-muted-foreground outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring aria-pressed:border-primary aria-pressed:bg-primary aria-pressed:text-primary-foreground roomy:h-touch roomy:px-4 roomy:text-base"
        onclick={() => {
          quickFilters.toggle(board, id, meId);
        }}
      >
        {QUICK_FILTER_NAMES[id]()}
      </button>
    {/each}
    {#if on.length > 0}
      <button
        type="button"
        data-quick-clear
        aria-label={m.viewheader_quick_clear_label({ count: on.length })}
        class="flex h-7 shrink-0 items-center gap-1 rounded-full px-2.5 text-sm whitespace-nowrap text-muted-foreground outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring roomy:h-touch roomy:px-3 roomy:text-base"
        onclick={(event) => {
          void clearAll(event.currentTarget.parentElement);
        }}
      >
        <XIcon class="size-4" aria-hidden="true" />
        {m.viewheader_quick_clear()}
        <span
          class="inline-flex h-4.5 min-w-4.5 items-center justify-center rounded-full bg-primary px-1 text-xs leading-none font-semibold text-primary-foreground"
          data-count
        >
          {on.length}
        </span>
      </button>
    {/if}
  </div>
{/if}
