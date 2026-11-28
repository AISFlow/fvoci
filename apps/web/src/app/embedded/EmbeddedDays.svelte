<!--
  A calendar view shown in a body: its cards by the day each begins on (the
  view's date property), those still to come or going on first, as a list.
-->
<script lang="ts">
  import type { CalendarDate } from '$lib/logic/dates.ts';
  import type { Card } from '$lib/logic/wire.ts';

  import EmbeddedCardLink from './EmbeddedCardLink.svelte';
  import EmbeddedValue from './EmbeddedValue.svelte';
  import { dayName, daysOf, type EmbeddedHost } from './embedded-view.ts';

  interface Props {
    host: EmbeddedHost;
    cards: readonly Card[];
    today: CalendarDate;
  }

  let { host, cards, today }: Props = $props();

  const days = $derived(
    daysOf(
      cards,
      host.view.fields.dateProperty,
      host.store.board?.cardProperties ?? [],
      today,
      host.regional.timeZone,
    ),
  );
</script>

<ol class="embedded-days" data-testid="embedded-days">
  {#each days as day (day.day)}
    <li>
      <h4 class="embedded-day-name">{dayName(day.day, host.locale)}</h4>
      <ul class="grid gap-1">
        {#each day.cards as card (card.id)}
          <li class="embedded-day-card">
            <EmbeddedCardLink {host} {card} />
            {#each host.columns as column (column.id)}
              {#if card.fields.properties[column.id] !== undefined}
                <EmbeddedValue {host} {card} template={column.template} />
              {/if}
            {/each}
          </li>
        {/each}
      </ul>
    </li>
  {/each}
</ol>
