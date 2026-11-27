<!--
  One condition of a filter: the property it is on, the condition, and what
  it compares with. What the last part is depends on the property's kind of
  value: options and people are ticked in a list, text is typed, a date is a
  day picked from a calendar, and a condition such as "is empty" compares
  with nothing.

  In a narrow place the line folds in two: property and condition, then the
  value and the button that removes the line.

  Text is saved when the field is left or Enter is pressed, and also when the
  line is taken off the screen with a text not saved yet (the surface it is on
  turned from a sheet into a popover): what was typed is not lost.
-->
<script lang="ts">
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import XIcon from '@lucide/svelte/icons/x';
  import { onDestroy } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { isComposing } from '$lib/keys.ts';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import PersonProperty from '$lib/components/properties/kinds/PersonProperty.svelte';
  import SelectProperty from '$lib/components/properties/kinds/SelectProperty.svelte';
  import type { PropertyUser } from '$lib/components/properties/types.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { filterConditionDisplayString } from '$lib/logic/filter-conditions.ts';
  import type { Regional } from '$lib/logic/regional.ts';
  import {
    DYNAMIC_FILTER_ME,
    type FilterClause,
    type FilterCondition,
    type PropertyTemplate,
  } from '$lib/logic/wire.ts';
  import { Input } from '$lib/ui/input/index.js';

  import {
    conditionsFor,
    filterableProperties,
    filterValueTypeOf,
    takesValue,
    TITLE_FILTER_ID,
  } from './filter-edit.ts';
  import FilterDate from './FilterDate.svelte';
  import { CONDITION_NAMES, propertyName } from './texts.ts';

  interface Props {
    clause: FilterClause;
    templates: readonly PropertyTemplate[];
    /** The people a condition on a person property can name: the members of the board. */
    people: readonly PropertyUser[];
    /** Who a value may name though not offered: an author who left, say. */
    named?: readonly PropertyUser[];
    locale: string;
    regional: Regional;
    onProperty: (propertyId: string) => void;
    onCondition: (condition: FilterCondition) => void;
    onValues: (values: string[]) => void;
    onRemove: () => void;
  }

  let {
    clause,
    templates,
    people,
    named,
    locale,
    regional,
    onProperty,
    onCondition,
    onValues,
    onRemove,
  }: Props = $props();

  const pickers = new PickerPresentation();

  const template = $derived(templates.find((candidate) => candidate.id === clause.propertyId));
  const name = $derived(propertyName(clause.propertyId, templates));
  const valueType = $derived(filterValueTypeOf(clause.propertyId, templates));
  const conditions = $derived(conditionsFor(valueType));
  const valueName = $derived(m.viewheader_filter_value_of({ property: name }));
  const peopleWithMe = $derived<readonly PropertyUser[]>([
    { id: DYNAMIC_FILTER_ME, username: DYNAMIC_FILTER_ME, displayName: m.filter_me() },
    ...people,
  ]);
  const withValue = $derived(valueType !== 'none' && takesValue(clause.condition));

  const propertyActions = $derived<MenuAction[]>(
    [TITLE_FILTER_ID, ...filterableProperties(templates).map((each) => each.id)].map((id) => ({
      id,
      label: propertyName(id, templates),
      checked: id === clause.propertyId,
      run: () => {
        onProperty(id);
      },
    })),
  );

  const conditionActions = $derived<MenuAction[]>(
    conditions.map((condition) => ({
      id: condition,
      label: CONDITION_NAMES[condition](),
      checked: condition === clause.condition,
      run: () => {
        onCondition(condition);
      },
    })),
  );

  // ------------------------------------------------------------- text

  const storedText = $derived(clause.values[0] ?? '');
  // The field follows the stored value; while a person types, the text is theirs.
  let draft = $derived(storedText);
  /** What was typed and not saved yet. Not reactive: it is read while the line is being removed. */
  let unsaved: string | undefined;

  function saveText(): void {
    if (unsaved === undefined) return;
    const text = unsaved;
    unsaved = undefined;
    if (text !== storedText) onValues(text === '' ? [] : [text]);
  }

  onDestroy(saveText);

  const choiceClass =
    'flex h-8 max-w-full min-w-0 shrink-0 items-center justify-between gap-1 rounded-lg border border-input-border px-2.5 text-left text-sm hover:bg-muted aria-expanded:bg-muted disabled:opacity-50 roomy:h-touch roomy:text-base';
</script>

{#snippet choice(props: Record<string, unknown>, label: string, text: string, slot: string)}
  <button
    {...props}
    type="button"
    data-slot={slot}
    class={choiceClass}
    aria-label="{label}: {text}"
  >
    <span class="min-w-0 truncate">{text}</span>
    <ChevronDownIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
  </button>
{/snippet}

<li class="flex flex-wrap items-center gap-2" data-filter-row={clause.propertyId}>
  <ActionMenu title={m.viewheader_filter_property()} actions={propertyActions} align="start">
    {#snippet trigger(props: Record<string, unknown>)}
      {@render choice(props, m.viewheader_filter_property(), name, 'filter-property')}
    {/snippet}
  </ActionMenu>
  <ActionMenu
    title={m.viewheader_filter_condition()}
    subtitle={name}
    actions={conditionActions}
    align="start"
  >
    {#snippet trigger(props: Record<string, unknown>)}
      {@render choice(
        { ...props, disabled: conditions.length === 0 },
        m.viewheader_filter_condition(),
        filterConditionDisplayString(clause.condition, CONDITION_NAMES, valueType),
        'filter-condition',
      )}
    {/snippet}
  </ActionMenu>
  <!-- The value asks for a width a phone does not have beside the two choices: there it takes a second line. -->
  <div class={['flex min-w-0 flex-1 items-center gap-1', withValue ? 'basis-56' : 'basis-10']}>
    <div class="min-w-0 flex-1" data-slot="filter-value">
      {#if !withValue}
        <!-- "is empty", "is set": nothing to compare with. -->
      {:else if valueType === 'options'}
        <div class="rounded-lg border border-input-border">
          <SelectProperty
            name={valueName}
            options={template?.options ?? []}
            ids={clause.values}
            multiple
            editable
            placeholder={m['FilterValue.empty']()}
            presentation={pickers.current}
            onChange={(value: string | string[]) => {
              onValues(typeof value === 'string' ? [value] : value);
            }}
          />
        </div>
      {:else if valueType === 'person'}
        <div class="rounded-lg border border-input-border">
          <PersonProperty
            name={valueName}
            ids={clause.values}
            multiple
            users={peopleWithMe}
            {named}
            editable
            placeholder={m['FilterValue.empty']()}
            presentation={pickers.current}
            onChange={(value: string | string[]) => {
              onValues(typeof value === 'string' ? [value] : value);
            }}
          />
        </div>
      {:else if valueType === 'date'}
        <FilterDate
          name={valueName}
          value={clause.values[0]}
          {locale}
          {regional}
          presentation={pickers.current}
          onChange={(value: string | undefined) => {
            onValues(value === undefined ? [] : [value]);
          }}
        />
      {:else}
        <Input
          bind:value={
            () => draft,
            (text: string) => {
              draft = text;
              unsaved = text;
            }
          }
          aria-label={valueName}
          placeholder={m['FilterByText.placeholder']()}
          autocomplete="off"
          enterkeyhint="done"
          class="roomy:h-touch roomy:text-base"
          onblur={saveText}
          onkeydown={(event: KeyboardEvent) => {
            // The Enter that confirms a syllable of an input method is not this Enter.
            if (event.key === 'Enter' && !isComposing(event)) saveText();
          }}
        />
      {/if}
    </div>
    <IconButton
      label={m.viewheader_filter_remove({ property: name })}
      data-remove
      onclick={onRemove}
    >
      <XIcon />
    </IconButton>
  </div>
</li>
