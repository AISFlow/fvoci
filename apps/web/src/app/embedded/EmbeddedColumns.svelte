<!--
  A board (kanban) view shown in a body: its columns as the board shows them
  (the view's grouping, its hidden columns left out), each with its cards in
  the view's order and the properties the view shows on a card. The columns
  scroll sideways within the view; the page does not.
-->
<script lang="ts">
  import OptionChip from '$lib/components/option-chip/OptionChip.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Card } from '$lib/logic/wire.ts';

  import EmbeddedCardLink from './EmbeddedCardLink.svelte';
  import EmbeddedValue from './EmbeddedValue.svelte';
  import type { EmbeddedHost } from './embedded-view.ts';

  interface Props {
    host: EmbeddedHost;
    cards: readonly Card[];
  }

  let { host, cards }: Props = $props();

  const groups = $derived(
    host.store.groupsOf(host.view, cards, (property) =>
      m['BoardComponent.no-property']({ property }),
    ).visible,
  );
</script>

<div class="embedded-columns" data-testid="embedded-columns">
  {#each groups as group (group.option.id)}
    <section class="embedded-column" aria-label={group.option.label}>
      <h4 class="embedded-column-name">
        <OptionChip name={group.option.label} color={group.option.color} />
        <span class="text-muted-foreground">{group.cards.length}</span>
      </h4>
      <ul class="grid gap-1.5">
        {#each group.cards as card (card.id)}
          <li class="embedded-tile">
            <EmbeddedCardLink {host} {card} class="font-medium" />
            {#each host.columns as column (column.id)}
              {#if card.fields.properties[column.id] !== undefined}
                <EmbeddedValue {host} {card} template={column.template} />
              {/if}
            {/each}
          </li>
        {/each}
      </ul>
    </section>
  {/each}
</div>
