<!--
  Under the value of a relation in a card's detail: the cards whose value
  of the same property names this card ("Cards that point to this card"),
  each a button that opens it. Nothing is drawn where there are none. The
  list is read from the cards of the board (referrers.ts); it is changed
  from the cards that hold the link, not here.

  The cards stand in one line that scrolls sideways: the list is as high at
  every width, so the card's text below it stays where it is when the frame
  of the card changes.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Card } from '$lib/logic/wire.ts';

  interface Props {
    referrers: readonly Card[];
    /** Opens a card. Without it the cards are only named. */
    onOpenCard?: (cardId: string) => void;
  }

  let { referrers, onOpenCard }: Props = $props();

  const uid = $props.id();
  const titleOf = (card: Card): string => card.title || m['KanbanCard.untitled']();
  const chipClass =
    'inline-flex max-w-full min-w-0 items-center gap-1 rounded-md bg-muted px-1.5 py-0.5 text-sm text-foreground roomy:min-h-touch';
</script>

{#if referrers.length > 0}
  <div data-slot="relation-referrers" class="flex min-w-0 flex-col gap-1 px-2 pt-0.5 pb-1.5">
    <span id="{uid}-label" class="truncate text-xs text-muted-foreground">
      {m.property_relation_referrers()}
    </span>
    <ul
      aria-labelledby="{uid}-label"
      class="m-0 flex [scrollbar-width:thin] list-none gap-1 overflow-x-auto overscroll-x-contain p-0"
    >
      {#each referrers as card (card.id)}
        <li class="max-w-56 min-w-0 shrink-0">
          {#if onOpenCard}
            <button
              type="button"
              data-slot="relation-referrer"
              data-card-id={card.id}
              class="{chipClass} underline-offset-2 hover:underline focus-visible:underline"
              aria-label={m.property_relation_open({ title: titleOf(card) })}
              onclick={() => {
                onOpenCard(card.id);
              }}
            >
              <span class="truncate">{titleOf(card)}</span>
            </button>
          {:else}
            <span data-slot="relation-referrer" data-card-id={card.id} class={chipClass}>
              <span class="truncate">{titleOf(card)}</span>
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
{/if}
