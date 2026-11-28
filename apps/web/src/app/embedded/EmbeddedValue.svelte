<!--
  The value of one property of one card in a board view shown in a body: a
  checkbox and the board's status are changed here by who may change cards
  (`changedHere`); anything else is only shown, and changed on the card.
-->
<script lang="ts">
  import { PropertyValue } from '$lib/components/properties/index.ts';
  import type { Card, PropertyTemplate, PropertyValue as StoredValue } from '$lib/logic/wire.ts';

  import { changedHere, type EmbeddedHost } from './embedded-view.ts';

  interface Props {
    host: EmbeddedHost;
    card: Card;
    template: PropertyTemplate;
  }

  let { host, card, template }: Props = $props();

  const here = $derived(changedHere(host, template));
</script>

{#if here}
  <PropertyValue
    {template}
    {card}
    value={card.fields.properties[template.id]}
    mode="edit"
    locale={host.locale}
    regional={host.regional}
    users={host.people}
    named={host.users}
    presentation={host.presentation}
    onChange={(value: StoredValue | undefined) => {
      void host.store.setCardProperty(card.id, template.id, value);
    }}
  />
{:else}
  <PropertyValue
    {template}
    {card}
    value={card.fields.properties[template.id]}
    mode="display"
    locale={host.locale}
    regional={host.regional}
    users={host.users}
  />
{/if}
