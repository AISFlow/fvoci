<!--
  The value of a select property (one option) or a multi-select property
  (several). Pressing the value opens the list of options: typing narrows
  it, and a name that no option has can be created on the spot.

  Each option has a button that opens its settings in place of the list: its
  name, its colour, and deleting it. Those change the property's definition,
  so they are only reported, through the callbacks; the caller applies them.

  An identifier in the value that no option has (the option was deleted) is
  not shown.
-->
<script lang="ts">
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import CheckIcon from '@lucide/svelte/icons/check';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import { tick } from 'svelte';

  import {
    OPTION_COLORS,
    optionColorName,
    optionFill,
  } from '$lib/components/option-chip/colors.ts';
  import { arrowChoice } from '$lib/components/arrow-choice.ts';
  import { isComposing } from '$lib/keys.ts';
  import OptionChip from '$lib/components/option-chip/OptionChip.svelte';
  import PickList from '$lib/components/pick-list/PickList.svelte';
  import type { Snippet } from 'svelte';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { PropertyOption } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { cn } from '$lib/utils.js';

  import type { OptionCallbacks } from '../types.ts';

  interface Props extends OptionCallbacks {
    /** The name of the property. */
    name: string;
    options: readonly PropertyOption[];
    /** The identifiers of the options in the value. */
    ids: readonly string[];
    multiple: boolean;
    editable: boolean;
    /** Shown where there is no value; empty for nothing. */
    placeholder: string;
    presentation?: Presentation;
    startOpen?: boolean;
    /**
     * Draws what opens the picker, in place of the value: a button of the bar
     * of a selection. `props` are spread on it, before its own attributes.
     */
    opener?: Snippet<[{ props: Record<string, unknown> }]>;
    /**
     * Where the focus goes once the picker closes, in place of an `opener`
     * that is not to be seen (`PickerSurface`); its own otherwise.
     */
    returnFocus?: () => HTMLElement | null | undefined;
    /**
     * The value stands for several cards whose values differ: none is shown
     * as chosen, and clearing is offered all the same (it clears them all).
     */
    mixed?: boolean;
    /** One identifier (or `''`) for a select, a list for a multi-select. */
    onChange?: (value: string | string[]) => void;
  }

  let {
    name,
    options,
    ids,
    multiple,
    editable,
    placeholder,
    presentation = 'popover',
    startOpen = false,
    opener,
    returnFocus,
    mixed = false,
    onChange,
    onCreateOption,
    onRenameOption,
    onRecolorOption,
    onDeleteOption,
  }: Props = $props();

  const uid = $props.id();
  // svelte-ignore state_referenced_locally (only where editing starts)
  let open = $state(startOpen);
  let search = $state('');
  /** The option whose settings are open, in place of the list. */
  let editingId = $state<string | undefined>(undefined);
  let nameDraft = $state('');

  const chosen = $derived(
    ids
      .map((id) => options.find((option) => option.id === id))
      .filter((option): option is PropertyOption => option !== undefined),
  );
  const chosenIds = $derived(chosen.map((option) => option.id));
  const editing = $derived(options.find((option) => option.id === editingId));
  const canEditOptions = $derived(
    onRenameOption !== undefined || onRecolorOption !== undefined || onDeleteOption !== undefined,
  );

  function pick(option: PropertyOption): void {
    if (!multiple) {
      if (chosenIds[0] !== option.id) onChange?.(option.id);
      open = false;
      return;
    }
    onChange?.(
      chosenIds.includes(option.id)
        ? chosenIds.filter((id) => id !== option.id)
        : [...chosenIds, option.id],
    );
  }

  function remove(id: string): void {
    onChange?.(multiple ? chosenIds.filter((other) => other !== id) : '');
  }

  function clear(): void {
    onChange?.(multiple ? [] : '');
    if (!multiple) open = false;
  }

  async function create(text: string): Promise<void> {
    if (!onCreateOption) return;
    // The value as it is now: the option may take a while to be made.
    const before = chosenIds;
    let option: PropertyOption;
    try {
      option = await onCreateOption(text);
    } catch {
      // The caller could not make the option and reports that itself; the value stays.
      return;
    }
    onChange?.(multiple ? [...before, option.id] : option.id);
    if (!multiple) open = false;
  }

  function editOption(option: PropertyOption): void {
    editingId = option.id;
    nameDraft = option.label;
  }

  async function backToList(): Promise<void> {
    const id = editingId;
    editingId = undefined;
    await tick();
    // Continue from the button that led here.
    if (id !== undefined) document.getElementById(`${uid}-edit-${id}`)?.focus();
  }

  function rename(): void {
    const text = nameDraft.trim();
    if (!editing) return;
    if (text === '' || text === editing.label) {
      nameDraft = editing.label;
      return;
    }
    onRenameOption?.(editing, text);
  }
</script>

{#snippet labels()}
  {#each chosen as option (option.id)}
    <OptionChip name={option.label} color={option.color} />
  {/each}
  {#if chosen.length === 0 && placeholder !== ''}
    <span data-slot="property-empty" class="text-muted-foreground">{placeholder}</span>
  {/if}
{/snippet}

{#if editable}
  <PickerSurface
    bind:open
    {presentation}
    title={name}
    {returnFocus}
    onOpenChange={(next) => {
      if (!next) {
        search = '';
        editingId = undefined;
      }
    }}
  >
    {#snippet trigger({ props }: { props: Record<string, unknown> })}
      {#if opener}
        {@render opener({ props })}
      {:else}
        <button
          {...props}
          type="button"
          data-slot="property-select"
          class="flex min-h-8 w-full min-w-0 flex-wrap items-center gap-1 rounded-md px-2 py-1 text-left hover:bg-muted aria-expanded:bg-muted roomy:min-h-touch"
        >
          <span class="sr-only">{name}</span>
          {@render labels()}
        </button>
      {/if}
    {/snippet}

    {#if editing}
      <div class="flex flex-col gap-2 p-1" data-slot="option-settings">
        <div class="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon-sm"
            class="group-data-[presentation=sheet]/surface:size-touch"
            aria-label={m.ui_back()}
            onclick={() => void backToList()}
          >
            <ArrowLeftIcon aria-hidden="true" />
          </Button>
          <Input
            bind:value={nameDraft}
            aria-label={m.property_option_name()}
            readonly={onRenameOption === undefined}
            autocomplete="off"
            enterkeyhint="done"
            class="group-data-[presentation=sheet]/surface:h-touch"
            onblur={rename}
            onkeydown={(event: KeyboardEvent) => {
              if (isComposing(event)) return;
              if (event.key === 'Enter') {
                event.preventDefault();
                rename();
              } else if (event.key === 'Escape' && nameDraft !== editing.label) {
                // The first Escape puts the name back; the next one closes.
                event.preventDefault();
                event.stopPropagation();
                nameDraft = editing.label;
              }
            }}
          />
        </div>
        {#if onRecolorOption}
          <div
            role="radiogroup"
            aria-label={m.property_option_color()}
            class="flex flex-col"
            data-slot="option-colors"
            {@attach arrowChoice({ choose: false })}
          >
            {#each OPTION_COLORS as color (color)}
              <button
                type="button"
                role="radio"
                aria-checked={editing.color === color}
                title={m['ColorOption.selectColor']({ color: optionColorName(color) })}
                class="flex min-h-8 items-center gap-2 rounded-sm px-2 py-1 text-left text-sm group-data-[presentation=sheet]/surface:min-h-touch hover:bg-muted"
                onclick={() => {
                  if (editing.color !== color) onRecolorOption(editing, color);
                }}
              >
                <span
                  class={cn(
                    'size-4 shrink-0 rounded-sm border border-foreground/15',
                    optionFill(color),
                  )}
                  aria-hidden="true"
                ></span>
                <span class="flex-1">{optionColorName(color)}</span>
                {#if editing.color === color}
                  <CheckIcon class="size-4 shrink-0" aria-hidden="true" />
                {/if}
              </button>
            {/each}
          </div>
        {/if}
        {#if onDeleteOption}
          <Button
            variant="destructive"
            class="justify-start group-data-[presentation=sheet]/surface:min-h-touch"
            onclick={() => {
              const option = editing;
              editingId = undefined;
              onDeleteOption(option);
            }}
          >
            <Trash2Icon aria-hidden="true" />
            {m['BoardComponent.delete']()}
          </Button>
        {/if}
      </div>
    {:else}
      <div class="flex flex-col gap-1">
        {#if multiple && chosen.length > 0}
          <ul class="m-0 flex list-none flex-wrap gap-1 p-1" aria-label={name}>
            {#each chosen as option (option.id)}
              <li class="flex min-w-0">
                <OptionChip
                  name={option.label}
                  color={option.color}
                  onRemove={() => {
                    remove(option.id);
                  }}
                />
              </li>
            {/each}
          </ul>
        {/if}
        <PickList
          bind:search
          items={options}
          textOf={(option: PropertyOption) => option.label}
          selected={chosenIds}
          {multiple}
          label={m['ValueSelector.valueSelector']()}
          placeholder={onCreateOption ? m.property_option_search() : m['ConfirmPerson.search']()}
          emptyText={onCreateOption && options.length === 0
            ? m['ValueSelector.noOptions']()
            : m.property_no_matches()}
          createText={(text: string) => m.property_option_create({ name: text })}
          onPick={pick}
          onCreate={onCreateOption ? (text: string) => void create(text) : undefined}
          onBackspaceEmpty={() => {
            const last = chosenIds.at(-1);
            if (multiple && last !== undefined) remove(last);
          }}
        >
          {#snippet row(option: PropertyOption)}
            <OptionChip name={option.label} color={option.color} />
          {/snippet}
          {#snippet trailing(option: PropertyOption)}
            {#if canEditOptions}
              <Button
                id="{uid}-edit-{option.id}"
                variant="ghost"
                size="icon-sm"
                class="shrink-0 group-data-[presentation=sheet]/surface:size-touch"
                aria-label={m.property_option_edit({ name: option.label })}
                title={m['ValueSelectorLabel.openMenu']()}
                onclick={() => {
                  editOption(option);
                }}
              >
                <EllipsisIcon aria-hidden="true" />
              </Button>
            {/if}
          {/snippet}
        </PickList>
        {#if chosen.length > 0 || mixed}
          <Button
            variant="ghost"
            class="justify-start group-data-[presentation=sheet]/surface:min-h-touch"
            onclick={clear}
          >
            {m.property_clear()}
          </Button>
        {/if}
      </div>
    {/if}
  </PickerSurface>
{:else}
  <span
    data-slot="property-select"
    class="flex min-h-8 min-w-0 flex-wrap items-center gap-1 px-2 py-1"
  >
    {@render labels()}
  </span>
{/if}
