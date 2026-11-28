<!--
  A table, a gallery or a gantt view shown in a body: its cards as rows, by
  the properties the view shows, in the view's order. The cards' titles stay
  at the left as the table scrolls sideways within itself; the page does not.
-->
<script lang="ts">
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
</script>

<table class="embedded-table" data-testid="embedded-rows">
  <thead>
    <tr>
      <th scope="col" class="embedded-first">{m['TableComponent.name']()}</th>
      {#each host.columns as column (column.id)}
        <th scope="col">{column.template.name}</th>
      {/each}
    </tr>
  </thead>
  <tbody>
    {#each cards as card (card.id)}
      <tr>
        <th scope="row" class="embedded-first">
          <EmbeddedCardLink {host} {card} />
        </th>
        {#each host.columns as column (column.id)}
          <td><EmbeddedValue {host} {card} template={column.template} /></td>
        {/each}
      </tr>
    {/each}
  </tbody>
</table>
