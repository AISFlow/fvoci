<!--
  The value of a text, number, email or phone property: one line of text,
  edited in place. The kinds differ in what they accept and in the keyboard a
  phone offers.
-->
<script lang="ts" module>
  import type { HTMLInputAttributes } from 'svelte/elements';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isEmailEntry, isNumberEntry } from '$lib/logic/property-entry.ts';

  export type TextKind = 'text' | 'number' | 'email' | 'phone';

  interface KindRules {
    validate: (text: string) => boolean;
    errorText?: () => string;
    inputmode: HTMLInputAttributes['inputmode'];
    spellcheck: boolean;
  }

  const KINDS: Record<TextKind, KindRules> = {
    text: { validate: () => true, inputmode: 'text', spellcheck: true },
    number: {
      validate: isNumberEntry,
      errorText: m.property_invalid_number,
      inputmode: 'decimal',
      spellcheck: false,
    },
    email: {
      validate: isEmailEntry,
      errorText: m.property_invalid_email,
      inputmode: 'email',
      spellcheck: false,
    },
    phone: { validate: () => true, inputmode: 'tel', spellcheck: false },
  };
</script>

<script lang="ts">
  import TextField from '../parts/TextField.svelte';

  interface Props {
    kind: TextKind;
    /** The name of the property. */
    name: string;
    value: string;
    editable: boolean;
    /** Shown where there is no value; empty for nothing. */
    placeholder: string;
    startOpen?: boolean;
    onChange: (value: string) => void;
  }

  let { kind, name, value, editable, placeholder, startOpen = false, onChange }: Props = $props();

  const rules = $derived(KINDS[kind]);
</script>

{#if editable}
  <TextField
    label={name}
    {value}
    {placeholder}
    validate={rules.validate}
    errorText={rules.errorText?.()}
    inputmode={rules.inputmode}
    spellcheck={rules.spellcheck}
    autofocus={startOpen}
    onSave={onChange}
  />
{:else if value !== ''}
  <span data-slot="property-text" class="block min-w-0 truncate px-2 py-1.5" title={value}
    >{value}</span
  >
{:else if placeholder !== ''}
  <span data-slot="property-empty" class="block px-2 py-1.5 text-muted-foreground"
    >{placeholder}</span
  >
{/if}
