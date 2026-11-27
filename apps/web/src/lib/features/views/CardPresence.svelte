<script lang="ts">
  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { boardPresence, type BoardStore } from '$lib/state/index.ts';

  let {
    board,
    cardId,
    class: className,
  }: { board: BoardStore; cardId: string; class?: string } = $props();
  /** Who else has the card open: O(1) indexed lookup with frozen empty identity (§1.2.1). */
  const people = $derived(boardPresence(board).forCard(cardId));
  const names = $derived(
    people
      .map((person) => board.workspace.userName(person.userId) ?? m['centerPanel.unknown-user']())
      .join(', '),
  );
  const label = $derived(m.presence_card_viewers({ count: people.length, names }));
</script>

{#if people.length > 0}
  <span
    data-testid="card-presence"
    role="img"
    aria-label={label}
    title={label}
    class={['flex shrink-0 items-center gap-1 text-xs text-muted-foreground', className]}
  >
    <span class="flex">
      {#each people.slice(0, 2) as person, index (person.userId)}
        <Avatar
          name={board.workspace.userName(person.userId) ?? m['centerPanel.unknown-user']()}
          colorKey={person.userId}
          locale={board.workspace.locale}
          decorative
          class={['size-4 ring-1 ring-background', index > 0 && '-ms-1']}
        />
      {/each}
    </span>
    {#if people.length > 2}<span aria-hidden="true">+{people.length - 2}</span>{/if}
  </span>
{/if}
