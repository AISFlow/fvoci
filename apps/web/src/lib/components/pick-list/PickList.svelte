<!--
  A list to pick from, with a field that narrows it down as the person types.

  The field keeps the focus; the arrow keys move a highlight through the
  list and Enter picks the highlighted row (the combobox pattern, so a screen
  reader hears the highlighted row through `aria-activedescendant`). With
  `onCreate`, a text that is not the name of any item offers a last row that
  creates it.

  A row is only the thing to pick. A control that belongs to a row but does
  something else (editing an option) goes into `trailing`, next to the row and
  not inside it.
-->
<script lang="ts" generics="Item extends { id: string }">
  import CheckIcon from '@lucide/svelte/icons/check';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import type { Snippet } from 'svelte';

  import { isComposing } from '$lib/keys.ts';
  import { foldCase } from '$lib/logic/text.ts';
  import { Input } from '$lib/ui/input/index.js';
  import { cn } from '$lib/utils.js';

  interface Props {
    items: readonly Item[];
    /** The texts an item is found by. */
    textOf: (item: Item) => string | readonly string[];
    /** The identifiers that are part of the value now; their rows are marked. */
    selected: readonly string[];
    multiple?: boolean;
    /** Names the field and the list. */
    label: string;
    placeholder?: string;
    /** Shown when there is nothing to pick and nothing to create. */
    emptyText: string;
    /** What the row that creates an item says, given the typed text. */
    createText?: (text: string) => string;
    row: Snippet<[Item]>;
    trailing?: Snippet<[Item]>;
    search?: string;
    onPick: (item: Item) => void;
    onCreate?: (text: string) => void;
    /** Backspace in the empty field: the usual way to drop the last value. */
    onBackspaceEmpty?: () => void;
  }

  let {
    items,
    textOf,
    selected,
    multiple = false,
    label,
    placeholder,
    emptyText,
    createText,
    row,
    trailing,
    search = $bindable(''),
    onPick,
    onCreate,
    onBackspaceEmpty,
  }: Props = $props();

  const uid = $props.id();
  const listId = `${uid}-list`;
  const CREATE = '\u0000create';

  const texts = (item: Item): readonly string[] => {
    const found = textOf(item);
    return typeof found === 'string' ? [found] : found;
  };

  const query = $derived(search.trim());
  const matches = $derived.by(() => {
    if (query === '') return items;
    const wanted = foldCase(query);
    return items.filter((item) => texts(item).some((text) => foldCase(text).includes(wanted)));
  });
  const canCreate = $derived(
    onCreate !== undefined &&
      query !== '' &&
      !items.some((item) => texts(item).some((text) => foldCase(text) === foldCase(query))),
  );
  /** The identifiers of the rows, top to bottom. */
  const rows = $derived([...matches.map((item) => item.id), ...(canCreate ? [CREATE] : [])]);

  let highlighted = $state<string | undefined>(undefined);
  /** The highlighted row, or the first one when the highlighted row is gone. */
  const active = $derived(
    highlighted !== undefined && rows.includes(highlighted) ? highlighted : rows[0],
  );

  const rowId = (id: string): string => `${uid}-row-${id === CREATE ? 'create' : id}`;

  let list = $state<HTMLElement | null>(null);

  function highlight(id: string | undefined): void {
    highlighted = id;
    if (id === undefined) return;
    const element = list?.ownerDocument.getElementById(rowId(id));
    // Not every environment lays out (and so scrolls) elements.
    if (typeof element?.scrollIntoView === 'function') element.scrollIntoView({ block: 'nearest' });
  }

  function move(step: number): void {
    if (rows.length === 0) return;
    const at = active === undefined ? -1 : rows.indexOf(active);
    highlight(rows[(at + step + rows.length) % rows.length]);
  }

  function pick(id: string | undefined): void {
    if (id === undefined) return;
    if (id === CREATE) {
      const text = query;
      search = '';
      onCreate?.(text);
      return;
    }
    const item = items.find((candidate) => candidate.id === id);
    if (!item) return;
    if (multiple) search = '';
    onPick(item);
  }

  function onkeydown(event: KeyboardEvent): void {
    // While an input method is composing (Hangul, kana), its keys are its own.
    if (isComposing(event)) return;
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      move(1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      move(-1);
    } else if (event.key === 'Home' && rows.length > 0 && search === '') {
      event.preventDefault();
      highlight(rows[0]);
    } else if (event.key === 'End' && rows.length > 0 && search === '') {
      event.preventDefault();
      highlight(rows.at(-1));
    } else if (event.key === 'Enter') {
      event.preventDefault();
      pick(active);
    } else if (event.key === 'Backspace' && search === '') {
      onBackspaceEmpty?.();
    }
  }

  const rowClass =
    'flex min-h-8 min-w-0 flex-1 cursor-default items-center gap-2 rounded-sm px-2 py-1 text-sm select-none group-data-[presentation=sheet]/surface:min-h-touch';
</script>

<div class="flex min-w-0 flex-col gap-1 p-1">
  <Input
    bind:value={search}
    role="combobox"
    aria-label={label}
    aria-expanded="true"
    aria-controls={listId}
    aria-autocomplete="list"
    aria-activedescendant={active === undefined ? undefined : rowId(active)}
    autocomplete="off"
    autocapitalize="off"
    spellcheck={false}
    enterkeyhint="done"
    data-picker-focus
    {placeholder}
    {onkeydown}
    class="group-data-[presentation=sheet]/surface:h-touch"
  />
  <ul
    bind:this={list}
    id={listId}
    role="listbox"
    aria-label={label}
    aria-multiselectable={multiple}
    class="m-0 max-h-72 list-none overflow-y-auto p-0 group-data-[presentation=sheet]/surface:max-h-[45dvh]"
  >
    {#each matches as item (item.id)}
      {@const chosen = selected.includes(item.id)}
      <li role="presentation" class="flex items-center gap-1">
        <!-- svelte-ignore a11y_click_events_have_key_events (the keys are handled by the field, which keeps the focus) -->
        <div
          id={rowId(item.id)}
          role="option"
          tabindex="-1"
          aria-selected={chosen}
          data-active={active === item.id ? '' : undefined}
          class={cn(rowClass, active === item.id && 'bg-muted')}
          onclick={() => {
            pick(item.id);
          }}
          onpointermove={() => {
            if (highlighted !== item.id) highlighted = item.id;
          }}
        >
          <span class="flex min-w-0 flex-1 items-center gap-2">{@render row(item)}</span>
          {#if chosen}<CheckIcon class="size-4 shrink-0" aria-hidden="true" />{/if}
        </div>
        {@render trailing?.(item)}
      </li>
    {/each}
    {#if canCreate}
      <li role="presentation" class="flex items-center">
        <!-- svelte-ignore a11y_click_events_have_key_events (the keys are handled by the field, which keeps the focus) -->
        <div
          id={rowId(CREATE)}
          role="option"
          tabindex="-1"
          aria-selected="false"
          data-active={active === CREATE ? '' : undefined}
          class={cn(rowClass, active === CREATE && 'bg-muted')}
          onclick={() => {
            pick(CREATE);
          }}
          onpointermove={() => {
            if (highlighted !== CREATE) highlighted = CREATE;
          }}
        >
          <PlusIcon class="size-4 shrink-0" aria-hidden="true" />
          <span class="min-w-0 truncate">{createText ? createText(query) : query}</span>
        </div>
      </li>
    {/if}
  </ul>
  {#if rows.length === 0}
    <p class="px-2 py-1.5 text-sm text-muted-foreground" role="status">{emptyText}</p>
  {/if}
</div>
