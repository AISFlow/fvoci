<script lang="ts">
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import RelationProperty from '$lib/components/properties/kinds/RelationProperty.svelte';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { relationLink, type RelationLink } from '$lib/logic/wire.ts';

  import { exampleCard } from '../fixtures.ts';

  let {
    count = 8,
    selected = 2,
    hidden = 0,
    loop = false,
    readOnly = false,
    startOpen = false,
  }: {
    count?: number;
    selected?: number;
    hidden?: number;
    loop?: boolean;
    readOnly?: boolean;
    startOpen?: boolean;
  } = $props();

  const cards = $derived(
    new Map(
      Array.from({ length: count }, (_, index) => {
        const card = exampleCard(index + 1);
        if (index === 0) card.title += ' — A long example title / 이름이 긴 예시 카드'.repeat(4);
        return [card.id, card];
      }),
    ),
  );
  let links: RelationLink[] = $derived([
    ...Array.from({ length: selected }, (_, index) => relationLink(`example-card-${index + 1}`)),
    ...Array.from({ length: hidden }, (_, index) => relationLink(`example-deleted-${index + 1}`)),
  ]);
  const excluded = $derived(loop ? new Set(['example-card-1', 'example-card-3']) : undefined);
  const surface = new PickerPresentation();
  let opened = $state('');
</script>

<div class="grid max-w-xl gap-4">
  <RelationProperty
    name={m.property_type_relation()}
    cardId="example-current-card"
    {cards}
    {links}
    {excluded}
    locale={getLocale()}
    editable={!readOnly}
    placeholder={m.property_relation_search()}
    presentation={surface.current}
    {startOpen}
    onChange={(value: RelationLink[]) => {
      links = value;
    }}
    onOpenCard={(id: string) => {
      opened = id;
    }}
  />
  <output data-testid="relation-value" class="text-xs break-all text-muted-foreground">
    {JSON.stringify(links)}
  </output>
  <output data-testid="opened-card" class="text-xs text-muted-foreground">{opened}</output>
</div>
