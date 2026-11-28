<!--
  The properties of a card: one line each, in the order of the board's
  definitions, the name and then the value. The value is the shared editor
  (lib/components/properties) and fills the rest of the line: the whole of it
  is what is pressed, not the text in it.

  The name is the way to the property's definition (PropertyMenu), and under
  the list a property can be added. Both change the board for every card. On
  a board the reader may not edit, names and values are only shown.

  The picker of the due date also says how the card repeats, and on a board
  that does not let cards repeat yet, holds the board's settings that do.
-->
<script lang="ts">
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { PropertyValue, type PropertyUser } from '$lib/components/properties/index.ts';
  import { referrersOf } from '$lib/components/properties/kinds/referrers.ts';
  import ReverseRelations from '$lib/components/properties/kinds/ReverseRelations.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import RecurrenceFields from '$lib/components/recurrence/RecurrenceFields.svelte';
  import BoardDueRules from '$lib/components/board-rules/BoardDueRules.svelte';
  import { overlayPresentation } from '$lib/layout/presentation.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import { orderingRelations } from '$lib/logic/gantt.ts';
  import { SELECTABLE_PROPERTY_TYPES } from '$lib/logic/properties.ts';
  import { boardLetsCardsRepeat, duePropertyOf, type Recurrence } from '$lib/logic/recurrence.ts';
  import {
    cardRecurrence,
    withCardRecurrence,
    type BoardView,
    type DoneRule,
    type PropertyType,
    type PropertyValue as StoredValue,
  } from '$lib/logic/wire.ts';
  import {
    setBoardDoneRule,
    setBoardDueProperty,
    type BoardStore,
    type CardSummary,
  } from '$lib/state/index.ts';
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
  /** The property the card is due by, whose picker says how it repeats. */
  const dueId = $derived(board.board ? duePropertyOf(board.board)?.id : undefined);
  const repeatable = $derived(board.board ? boardLetsCardsRepeat(board.board) : false);

  function setRecurrence(recurrence: Recurrence | undefined): void {
    void board.changeCard(card.id, (each) => withCardRecurrence(each, recurrence), 'setRecurrence');
  }
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

{#snippet repeatFields()}
  <RecurrenceFields
    recurrence={cardRecurrence(card)}
    allowed={repeatable}
    onChange={setRecurrence}
  />
  {#if !repeatable && board.canManageProperties && board.board && !board.board.isTemplate}
    <!-- The board's settings that let it, for who may set them. -->
    <BoardDueRules
      properties={board.board.cardProperties}
      dueDatePropertyId={board.board.dueDatePropertyId}
      doneWhen={board.board.doneWhen}
      onDueProperty={(propertyId: string | undefined) => {
        setBoardDueProperty(board, propertyId);
      }}
      onDoneRule={(rule: DoneRule | undefined) => {
        setBoardDoneRule(board, rule);
      }}
    />
  {/if}
{/snippet}

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
            dateExtra={template.id === dueId ? repeatFields : undefined}
          />
          {#if template.type === 'relation'}
            <ReverseRelations
              referrers={referrersOf(board.cardsById.values(), template.id, card.id)}
              {onOpenCard}
            />
          {/if}
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
