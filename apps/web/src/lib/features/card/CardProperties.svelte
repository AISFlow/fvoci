<!--
  The properties of a card: one line each, in the order of the board's
  definitions, the name and then the value. The value is the shared editor
  (lib/components/properties) and fills the rest of the line: the whole of it
  is what is pressed, not the text in it.

  The name is the way to the property's definition (PropertyMenu), and under
  the list a property can be added. Both change the board for every card. On
  a board the reader may not edit, names and values are only shown.
-->
<script lang="ts">
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { PropertyValue, type PropertyUser } from '$lib/components/properties/index.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { overlayPresentation } from '$lib/layout/presentation.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import { orderingRelations } from '$lib/logic/gantt.ts';
  import { SELECTABLE_PROPERTY_TYPES } from '$lib/logic/properties.ts';
  import type { BoardView, PropertyType, PropertyValue as StoredValue } from '$lib/logic/wire.ts';
  import type { BoardStore, CardSummary } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { propertyTypeIcon } from './icons.ts';
  import { addProperty, optionCallbacks } from './property-actions.ts';
  import { propertyTypeName } from './property-names.ts';
  import PropertyMenu from './PropertyMenu.svelte';

  interface Props {
    board: BoardStore;
    /** The view the card was opened from: a table gets a new property as a column. */
    view: BoardView | undefined;
    card: CardSummary;
    /** Opens another card: a card a relation names. */
    onOpenCard?: (cardId: string) => void;
  }

  let { board, view, card, onOpenCard }: Props = $props();

  const workspace = $derived(board.workspace);
  const templates = $derived(board.board?.cardProperties ?? []);
  const editable = $derived(board.canEdit);
  /** The relations a Gantt view reads as an order: no card that closes a loop is offered. */
  const ordering = $derived(
    orderingRelations(
      board.views.map((each) => each.fields),
      templates,
    ),
  );
  const presentation = $derived(overlayPresentation(page.stage));
  /** The people a person property can be set to: the members of the board. */
  const users = $derived<readonly PropertyUser[]>(board.people);
  /** Who a value may name: everyone the workspace knows, authors who left among them. */
  const named = $derived<readonly PropertyUser[]>(workspace.users);

  let addOpen = $state(false);
  /** The property that was just added: its menu opens, so that it can be named. */
  let added = $state<string | undefined>(undefined);

  async function add(type: PropertyType): Promise<void> {
    added = await addProperty(board, view, type, propertyTypeName(type));
  }

  const typeActions = $derived<MenuAction[]>(
    SELECTABLE_PROPERTY_TYPES.map((type) => ({
      id: type,
      label: propertyTypeName(type),
      icon: propertyTypeIcon(type),
      run: () => void add(type),
    })),
  );
</script>

<section aria-label={m['ViewHeader.properties']()} data-testid="card-properties">
  <dl class="flex flex-col">
    {#each templates as template (template.id)}
      {@const Icon = propertyTypeIcon(template.type)}
      {@const options = optionCallbacks(board, template.id)}
      <div
        class="grid grid-cols-[7.5rem_minmax(0,1fr)] items-start gap-x-1 @card-roomy/card:grid-cols-[10rem_minmax(0,1fr)]"
        data-property={template.id}
        data-property-type={template.type}
      >
        <dt class="min-w-0">
          {#if editable}
            <PropertyMenu {board} {template} {presentation} startOpen={template.id === added} />
          {:else}
            <span
              class="flex min-h-8 items-center gap-2 px-1 text-muted-foreground roomy:min-h-touch"
            >
              <Icon class="size-4 shrink-0" />
              <span class="truncate">{template.name}</span>
            </span>
          {/if}
        </dt>
        <dd class="flex min-h-8 min-w-0 flex-col justify-center roomy:min-h-touch">
          <PropertyValue
            {template}
            {card}
            value={card.fields.properties[template.id]}
            readOnly={!editable}
            showEmptyPlaceholder
            {users}
            {named}
            locale={workspace.locale}
            regional={workspace.regional}
            {presentation}
            cards={board.cardsById}
            preventLoops={ordering.has(template.id)}
            {onOpenCard}
            onChange={(value: StoredValue | undefined) =>
              void board.setCardProperty(card.id, template.id, value)}
            onCreateOption={options.onCreateOption}
            onRenameOption={options.onRenameOption}
            onRecolorOption={options.onRecolorOption}
            onDeleteOption={options.onDeleteOption}
          />
        </dd>
      </div>
    {/each}
  </dl>
  {#if editable}
    <ActionMenu
      bind:open={addOpen}
      title={m['PropertyMenu.selectType']()}
      align="start"
      actions={typeActions}
    >
      {#snippet trigger(props: Record<string, unknown>)}
        <Button
          {...props}
          variant="ghost"
          class="mt-1 -ml-1 text-muted-foreground roomy:min-h-touch"
          data-testid="add-property"
        >
          {m['CardDetail.add-property']()}
        </Button>
      {/snippet}
    </ActionMenu>
  {/if}
</section>
