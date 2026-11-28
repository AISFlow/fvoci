<!--
  The title of a card of a board view shown in a body, as a link to the card:
  a plain press opens the card over the page (`host.open`), and a press that
  asks for another tab, or a copy of the link, gets the card's address.
-->
<script lang="ts">
  import { followedHere } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Card } from '$lib/logic/wire.ts';

  import type { EmbeddedHost } from './embedded-view.ts';

  interface Props {
    host: EmbeddedHost;
    card: Card;
    class?: string;
  }

  let { host, card, class: className }: Props = $props();
</script>

<a
  href={host.cardHref(card.id)}
  class={['embedded-card-link', className]}
  data-testid="embedded-card"
  onclick={(event: MouseEvent) => {
    if (!followedHere(event)) return;
    event.preventDefault();
    host.open(card.id);
  }}
>
  {#if card.fields.icon}<span aria-hidden="true">{card.fields.icon}</span>{/if}
  <span class="truncate">{card.title || m['KanbanCard.untitled']()}</span>
</a>
