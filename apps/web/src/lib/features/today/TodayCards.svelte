<!--
  The cards the hub of today names, by board: each opened by its title, and
  made done at a press where its board says what done is. A board that does
  not say offers the choices of its properties in its heading, to whoever may
  choose.
-->
<script lang="ts">
  import CheckIcon from '@lucide/svelte/icons/check';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { doneChoices } from '$lib/logic/done-rule.ts';
  import type { TodayCardItem, TodayHubStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    store: TodayHubStore;
    /** The space a card is in, as the hub names it. */
    spaceOf: (card: TodayCardItem) => string;
    onOpen: (card: TodayCardItem) => void;
  }

  let { store, spaceOf, onOpen }: Props = $props();

  const groups = $derived.by(() => {
    const cards = store.data?.cards ?? [];
    return cards
      .filter((card, at) => cards.findIndex((one) => one.boardId === card.boardId) === at)
      .map((first) => ({
        first,
        cards: cards.filter((card) => card.boardId === first.boardId),
      }));
  });

  function dueOf(card: TodayCardItem): string {
    if (card.overdueDays > 0) return m.today_hub_card_late({ days: card.overdueDays });
    return card.dueDate === store.data?.today ? m.today_hub_card_due_today() : card.dueDate;
  }
</script>

{#if groups.length > 0}
  <section class="grid gap-3" data-testid="today-hub-cards">
    <h3 class="text-sm font-semibold text-muted-foreground">{m.today_hub_cards()}</h3>
    {#each groups as group (group.first.boardId)}
      {@const read = store.board(group.first.boardId)}
      {@const choices =
        read && !read.board.doneWhen && read.canChooseDone
          ? doneChoices(read.board.cardProperties)
          : []}
      <div class="grid gap-1.5" data-testid="today-hub-board">
        <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span class="text-sm font-medium">
            {group.first.boardTitle || m['Sidebar.untitled-board']()}
          </span>
          <span class="text-xs text-muted-foreground">{spaceOf(group.first)}</span>
          {#if choices.length > 0}
            <span class="flex flex-wrap items-center gap-1.5" data-testid="today-hub-done-pick">
              <span class="text-xs text-muted-foreground">{m.today_hub_done_pick()}</span>
              {#each choices as choice (choice.value)}
                <Button
                  variant="outline"
                  size="xs"
                  class="roomy:min-h-touch"
                  title={choice.label}
                  onclick={() => void store.chooseDone(group.first.boardId, choice.rule)}
                >
                  {choice.short}
                </Button>
              {/each}
            </span>
          {/if}
        </div>
        <ul class="grid gap-1.5">
          {#each group.cards as card (card.cardId)}
            {@const title = card.title || m['KanbanCard.untitled']()}
            <li class="flex items-center gap-1 rounded-md border pr-1">
              <button
                type="button"
                class="flex min-w-0 flex-1 items-center justify-between gap-3 rounded-md px-3 py-2 text-left hover:bg-accent/50 roomy:min-h-touch"
                onclick={() => {
                  onOpen(card);
                }}
              >
                <span class="min-w-0 flex-1 truncate font-medium">
                  {card.icon ? `${card.icon} ` : ''}{title}
                </span>
                <span
                  class={[
                    'shrink-0 text-xs',
                    card.overdueDays > 0 ? 'text-destructive' : 'text-muted-foreground',
                  ]}
                >
                  {dueOf(card)}
                </span>
              </button>
              {#if store.canMarkDone(card.boardId)}
                <IconButton
                  label={m.today_hub_card_done({ title })}
                  data-testid="today-hub-card-done"
                  onclick={() => void store.markDone(card)}
                >
                  <CheckIcon />
                </IconButton>
              {/if}
            </li>
          {/each}
        </ul>
      </div>
    {/each}
  </section>
{/if}
