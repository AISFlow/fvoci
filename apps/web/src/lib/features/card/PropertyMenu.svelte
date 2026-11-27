<!--
  The name of a property in a card, and behind it the menu of the property's
  definition: its name, its kind, deleting it. These change the board, not
  the card: every card of the board has the property.

  The menu opens on the same surface as every picker: a popover next to the
  name, or a sheet on a compact page. It has two pages, the second being the
  list of kinds.

  - The name is stored on Enter, when the field is left and when the menu
    closes. An empty name is not a name: the stored one comes back.
  - Changing the kind asks first when cards have a value: it says how many
    values stay and how many are removed (property-change.ts has the rules).
  - Moving it up or down changes the order of the board's properties, which
    every card and the view's panel show (`BoardStore.moveProperty`). At an
    end the button that cannot move it gives the focus to the other one.
  - Deleting always asks.
-->
<script lang="ts">
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
  import CheckIcon from '@lucide/svelte/icons/check';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import { untrack } from 'svelte';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import { isComposing } from '$lib/keys.ts';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { SELECTABLE_PROPERTY_TYPES } from '$lib/logic/properties.ts';
  import type { PropertyTemplate, PropertyType } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';

  import { propertyTypeIcon } from './icons.ts';
  import { changePropertyType, deleteProperty, renameProperty } from './property-actions.ts';
  import { planTypeChange, type TypeChangePlan } from './property-change.ts';
  import { propertyTypeName } from './property-names.ts';

  interface Props {
    board: BoardStore;
    template: PropertyTemplate;
    presentation: Presentation;
    /** Start with the menu open: the property was just added and wants a name. */
    startOpen?: boolean;
  }

  let { board, template, presentation, startOpen = false }: Props = $props();

  let open = $state(false);
  // Asked to open: at the start, or a moment later when the board has taken the new property.
  $effect(() => {
    if (startOpen) open = true;
  });
  let listingTypes = $state(false);
  let nameDraft = $state(untrack(() => template.name));
  let confirmingDelete = $state(false);
  let confirmingType = $state(false);
  /** The change of kind that waits for the reader's word, and the question that is asked about it. */
  let plan = $state.raw<TypeChangePlan | undefined>(undefined);
  let typeQuestion = $state('');

  const Icon = $derived(propertyTypeIcon(template.type));
  const order = $derived(board.board?.cardProperties.map((each) => each.id) ?? []);
  const place = $derived(order.indexOf(template.id));

  let moveUp = $state<HTMLButtonElement>();
  let moveDown = $state<HTMLButtonElement>();

  /** A move not shown yet, whose button may then be off: not state, read once. */
  let pending: { step: -1 | 1; from: number } | undefined;

  function move(step: -1 | 1): void {
    pending = { step, from: place };
    void board.moveProperty(template.id, step);
  }

  // Once the move shows at an end, the button pressed is off and would lose the focus: the other one takes it.
  $effect(() => {
    // Read whether or not a move waits, so that the effect runs again when the place changes.
    const now = place;
    if (!pending || now === pending.from) return;
    const { step } = pending;
    pending = undefined;
    if (step === -1 && place <= 0) moveDown?.focus();
    else if (step === 1 && place >= order.length - 1) moveUp?.focus();
  });

  // A name that changes elsewhere replaces the text, unless the menu is open on it.
  $effect(() => {
    const stored = template.name;
    if (!untrack(() => open)) nameDraft = stored;
  });

  function rename(): void {
    const name = nameDraft.trim();
    if (name === '') {
      nameDraft = template.name;
      return;
    }
    void renameProperty(board, template.id, name);
  }

  function chooseType(type: PropertyType): void {
    rename();
    open = false;
    listingTypes = false;
    if (type === template.type) return;
    // Every card of the board, the templates for new cards included.
    const next = planTypeChange(template, type, board.cards);
    if (next.kept + next.lost === 0) {
      void changePropertyType(board, next);
      return;
    }
    plan = next;
    const question = m['CardDetailProperty.confirm-property-name-change-subtext']({
      propertyName: template.name,
      customText: m['CardDetailProperty.property-name-change-subtext']({
        oldPropType: propertyTypeName(template.type),
        newPropType: propertyTypeName(type),
      }),
      numOfCards: next.kept + next.lost,
    });
    typeQuestion = `${question} ${m.card_property_type_outcome({ kept: next.kept, lost: next.lost })}`;
    confirmingType = true;
  }

  const ROW =
    'flex min-h-8 w-full items-center gap-2 rounded-sm px-2 py-1 text-left group-data-[presentation=sheet]/surface:min-h-touch group-data-[presentation=sheet]/surface:text-base hover:bg-muted';
</script>

<PickerSurface
  bind:open
  {presentation}
  title={m.card_property_menu({ name: template.name })}
  class="w-64 max-w-full group-data-[presentation=sheet]/surface:w-auto"
  onOpenChange={(next) => {
    if (next) return;
    rename();
    listingTypes = false;
  }}
>
  {#snippet trigger({ props }: { props: Record<string, unknown> })}
    <button
      {...props}
      type="button"
      data-testid="property-name"
      class="flex min-h-8 w-full min-w-0 items-center gap-2 rounded-md px-1 text-left text-muted-foreground hover:bg-muted aria-expanded:bg-muted roomy:min-h-touch"
    >
      <Icon class="size-4 shrink-0" />
      <span class="truncate">{template.name}</span>
    </button>
  {/snippet}

  {#if listingTypes}
    <div class="flex flex-col gap-1 p-1" data-testid="property-types">
      <div class="flex items-center gap-1">
        <Button
          variant="ghost"
          size="icon-sm"
          class="group-data-[presentation=sheet]/surface:size-touch"
          aria-label={m.ui_back()}
          onclick={() => {
            listingTypes = false;
          }}
        >
          <ArrowLeftIcon aria-hidden="true" />
        </Button>
        <p class="min-w-0 flex-1 truncate font-medium">{m['PropertyMenu.changeType']()}</p>
      </div>
      <ul role="menu" aria-label={m['PropertyMenu.changeType']()} class="flex flex-col">
        {#each SELECTABLE_PROPERTY_TYPES as type (type)}
          {@const TypeIcon = propertyTypeIcon(type)}
          <li role="none">
            <button
              type="button"
              role="menuitemradio"
              aria-checked={type === template.type}
              data-type={type}
              class={ROW}
              onclick={() => {
                chooseType(type);
              }}
            >
              <TypeIcon class="size-4 shrink-0 text-muted-foreground" />
              <span class="min-w-0 flex-1 truncate">{propertyTypeName(type)}</span>
              {#if type === template.type}
                <CheckIcon class="size-4 shrink-0" aria-hidden="true" />
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    </div>
  {:else}
    <div class="flex flex-col gap-1 p-1" data-testid="property-menu">
      <Input
        bind:value={nameDraft}
        aria-label={m.card_property_name()}
        autocomplete="off"
        enterkeyhint="done"
        class="group-data-[presentation=sheet]/surface:h-touch roomy:text-base"
        onblur={rename}
        onkeydown={(event: KeyboardEvent) => {
          if (isComposing(event)) return;
          if (event.key === 'Enter') {
            event.preventDefault();
            rename();
            open = false;
          } else if (event.key === 'Escape' && nameDraft !== template.name) {
            // The first Escape puts the name back; the next one closes the menu.
            event.preventDefault();
            event.stopPropagation();
            nameDraft = template.name;
          }
        }}
      />
      <button
        type="button"
        data-action="type"
        class={ROW}
        onclick={() => {
          listingTypes = true;
        }}
      >
        <Icon class="size-4 shrink-0 text-muted-foreground" />
        <span class="min-w-0 flex-1 truncate">{m['PropertyMenu.typeTitle']()}</span>
        <span class="shrink-0 text-muted-foreground">{propertyTypeName(template.type)}</span>
        <ChevronRightIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      </button>
      <button
        type="button"
        data-action="move-up"
        bind:this={moveUp}
        class={[ROW, 'disabled:opacity-50']}
        disabled={place <= 0}
        onclick={() => {
          move(-1);
        }}
      >
        <ArrowUpIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate">{m.card_property_move_up()}</span>
      </button>
      <button
        type="button"
        data-action="move-down"
        bind:this={moveDown}
        class={[ROW, 'disabled:opacity-50']}
        disabled={place < 0 || place >= order.length - 1}
        onclick={() => {
          move(1);
        }}
      >
        <ArrowDownIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate">{m.card_property_move_down()}</span>
      </button>
      <button
        type="button"
        data-action="delete"
        class={[ROW, 'text-destructive']}
        onclick={() => {
          open = false;
          confirmingDelete = true;
        }}
      >
        <Trash2Icon class="size-4 shrink-0" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate">{m['PropertyMenu.Delete']()}</span>
      </button>
    </div>
  {/if}
</PickerSurface>

<ConfirmDialog
  bind:open={confirmingDelete}
  title={m['CardDetailProperty.confirm-delete-heading']()}
  description={m['CardDetailProperty.confirm-delete-subtext']({ propertyName: template.name })}
  confirmLabel={m['CardDetailProperty.delete-action-button']()}
  destructive
  onConfirm={() => void deleteProperty(board, template.id)}
/>

<ConfirmDialog
  bind:open={confirmingType}
  title={m['CardDetailProperty.confirm-property-type-change']()}
  description={typeQuestion}
  confirmLabel={m['CardDetailProperty.property-change-action-button']()}
  destructive={(plan?.lost ?? 0) > 0}
  onConfirm={() => {
    if (plan) void changePropertyType(board, plan);
  }}
/>
