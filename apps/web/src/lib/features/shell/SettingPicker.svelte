<!--
  One setting with many answers (a time zone, a language): a button that
  shows the answer and opens a list with a field to search it. The list
  opens the way every picker does: a popover, or a sheet on a compact page
  (components/picker-surface).
-->
<script lang="ts">
  import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down';

  import PickList from '$lib/components/pick-list/PickList.svelte';
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';

  interface Item {
    id: string;
    label: string;
    /** More texts the item is found by, such as the tag of a language. */
    also?: readonly string[];
  }

  interface Props {
    /** Names the setting in the markup, for tests: `data-setting`. */
    name: string;
    label: string;
    /** Names the search field and is its placeholder. */
    searchLabel: string;
    items: readonly Item[];
    /** The identifier of the answer chosen now. */
    value: string;
    onPick: (id: string) => void;
  }

  let { name, label, searchLabel, items, value, onPick }: Props = $props();

  const uid = $props.id();
  const presentation = new PickerPresentation();

  let open = $state(false);
  let search = $state('');

  const chosen = $derived(items.find((item) => item.id === value));
</script>

<div class="grid gap-1.5" data-setting={name}>
  <span id="{uid}-label" class="font-medium">{label}</span>
  <PickerSurface
    bind:open
    presentation={presentation.current}
    title={label}
    onOpenChange={(now: boolean) => {
      if (!now) search = '';
    }}
  >
    {#snippet trigger({ props }: { props: Record<string, unknown> })}
      <button
        {...props}
        type="button"
        aria-labelledby="{uid}-label {uid}-value"
        class="flex min-h-8 w-full items-center gap-2 rounded-lg border border-input-border bg-transparent px-2.5 py-1 text-left hover:bg-muted aria-expanded:bg-muted dark:bg-input/30 roomy:min-h-touch"
      >
        <span id="{uid}-value" class="min-w-0 flex-1 truncate">{chosen?.label ?? value}</span>
        <ChevronsUpDownIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      </button>
    {/snippet}
    <PickList
      {items}
      textOf={(item: Item) => [item.label, ...(item.also ?? [])]}
      selected={[value]}
      label={searchLabel}
      placeholder={searchLabel}
      emptyText={m.property_no_matches()}
      bind:search
      onPick={(item: Item) => {
        open = false;
        search = '';
        if (item.id !== value) onPick(item.id);
      }}
    >
      {#snippet row(item: Item)}
        <span class="min-w-0 truncate">{item.label}</span>
      {/snippet}
    </PickList>
  </PickerSurface>
</div>
