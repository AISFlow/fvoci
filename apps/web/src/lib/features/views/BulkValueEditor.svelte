<!--
  The editor of one property in the bar of a selection: the editor a card
  has for it (lib/components/properties/kinds), opened from a button of the
  bar (`opener`) instead of from the value. What is picked is put on every
  selected card.

  The kinds that pick (options, people, a date) open their own surface. The
  others are a field or a box, which are put on a surface here. Where the
  selected cards' values differ (`shared.mixed`) nothing shows as chosen, and
  "Clear" clears them all. Closed, the focus goes back to the opener, or
  where `returnFocus` says when the opener is not to be seen.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
  import type { PropertyUser } from '$lib/components/properties/index.ts';
  import CheckboxProperty from '$lib/components/properties/kinds/CheckboxProperty.svelte';
  import DateProperty from '$lib/components/properties/kinds/DateProperty.svelte';
  import PersonProperty from '$lib/components/properties/kinds/PersonProperty.svelte';
  import SelectProperty from '$lib/components/properties/kinds/SelectProperty.svelte';
  import TextProperty from '$lib/components/properties/kinds/TextProperty.svelte';
  import UrlProperty from '$lib/components/properties/kinds/UrlProperty.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Regional } from '$lib/logic/regional.ts';
  import {
    plainPropertyValue,
    type PropertyTemplate,
    type PropertyValue,
  } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';

  import type { SharedValue } from './bulk-bar.ts';

  interface Props {
    template: PropertyTemplate;
    shared: SharedValue;
    /** Whom a person property can be set to: the members of the board. */
    users: readonly PropertyUser[];
    /** Who a value may name though not offered. */
    named: readonly PropertyUser[];
    locale: string;
    regional: Regional;
    presentation: Presentation;
    /** Opens as soon as it is drawn: it was chosen from the bar's menu. */
    startOpen?: boolean;
    opener: Snippet<[{ props: Record<string, unknown> }]>;
    /** Where the focus goes once the editor closes, when `opener` is not to be seen. */
    returnFocus?: () => HTMLElement | null | undefined;
    /** The value for every selected card; undefined or empty clears it. */
    onChange: (value: PropertyValue | undefined) => void;
  }

  let {
    template,
    shared,
    users,
    named,
    locale,
    regional,
    presentation,
    startOpen = false,
    opener,
    returnFocus,
    onChange,
  }: Props = $props();

  const type = $derived(template.type);
  /** The heading of the picker: it says where the values differ, which the button may only hint at. */
  const name = $derived(shared.mixed ? m.bulk_value_mixed({ name: template.name }) : template.name);
  const plain = $derived(plainPropertyValue(shared.value));
  const text = $derived(Array.isArray(plain) ? plain.join(',') : (plain ?? ''));
  const one = $derived(typeof plain === 'string' && plain !== '' ? [plain] : []);
  const many = $derived(Array.isArray(plain) ? [...new Set(plain)] : []);

  /** The surface of a field or a box; the kinds that pick have their own. */
  // svelte-ignore state_referenced_locally (only where it is drawn to open)
  let open = $state(startOpen);

  /**
   * "Clear" was pressed. The field saves what it holds when it is left, and
   * the surface closing leaves it: that text is not put over the clearing.
   */
  let clearing = false;

  function put(value: PropertyValue | undefined): void {
    if (clearing && value !== undefined && value !== '') return;
    open = false;
    onChange(value);
  }

  function clear(): void {
    clearing = true;
    put(undefined);
  }
</script>

{#if type === 'select' || type === 'multiSelect'}
  <SelectProperty
    {name}
    options={template.options}
    ids={type === 'select' ? one : many}
    multiple={type === 'multiSelect'}
    editable
    placeholder=""
    {presentation}
    {startOpen}
    {opener}
    {returnFocus}
    mixed={shared.mixed}
    {onChange}
  />
{:else if type === 'person' || type === 'multiPerson'}
  <PersonProperty
    {name}
    ids={type === 'person' ? one : many}
    multiple={type === 'multiPerson'}
    {users}
    {named}
    editable
    placeholder=""
    {presentation}
    {startOpen}
    {opener}
    {returnFocus}
    mixed={shared.mixed}
    {onChange}
  />
{:else if type === 'date'}
  <DateProperty
    {name}
    value={shared.value}
    editable
    placeholder=""
    {locale}
    {regional}
    {presentation}
    {startOpen}
    {opener}
    {returnFocus}
    mixed={shared.mixed}
    {onChange}
  />
{:else}
  <PickerSurface
    bind:open
    {presentation}
    title={name}
    trigger={opener}
    {returnFocus}
    onOpenChange={(next: boolean) => {
      if (next) clearing = false;
    }}
  >
    <div class="flex min-w-56 flex-col gap-1 p-1" data-slot="bulk-field">
      {#if type === 'checkbox'}
        <div class="flex items-center gap-1">
          <CheckboxProperty name={template.name} value={text} editable onChange={put} />
          <span class="text-sm">{template.name}</span>
        </div>
      {:else if type === 'url'}
        <UrlProperty
          {name}
          value={text}
          editable
          placeholder={shared.mixed ? m.bulk_mixed() : m['PropertyValueElement.empty']()}
          startOpen
          onChange={put}
        />
      {:else if type === 'text' || type === 'number' || type === 'email' || type === 'phone'}
        <TextProperty
          kind={type}
          {name}
          value={text}
          editable
          placeholder={shared.mixed ? m.bulk_mixed() : m['PropertyValueElement.empty']()}
          startOpen
          onChange={put}
        />
      {/if}
      {#if text !== '' || shared.mixed}
        <Button
          variant="ghost"
          class="justify-start group-data-[presentation=sheet]/surface:min-h-touch"
          onmousedown={(event: MouseEvent) => {
            // The focus stays in the field: leaving it would save its text and close the surface first.
            event.preventDefault();
          }}
          onclick={clear}
        >
          {m.property_clear()}
        </Button>
      {/if}
    </div>
  </PickerSurface>
{/if}
