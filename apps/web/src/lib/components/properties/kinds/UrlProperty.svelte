<!--
  The value of a URL property. With a value it is a link that opens in a new
  tab, a button that copies the address and, for someone who may edit, a
  button that turns it into a field. Without a value it is the field at once.

  Only an address with an allowed scheme becomes a link (the same rule as
  links in a card's body); anything else is shown as plain text.
-->
<script lang="ts">
  import CheckIcon from '@lucide/svelte/icons/check';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import { onDestroy, tick } from 'svelte';

  import { isAllowedHref } from '$lib/editor/schema.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isUrlEntry } from '$lib/logic/property-entry.ts';
  import { ensureProtocol } from '$lib/logic/text.ts';
  import { Button } from '$lib/ui/button/index.js';

  import TextField from '../parts/TextField.svelte';

  interface Props {
    /** The name of the property. */
    name: string;
    value: string;
    editable: boolean;
    /** Shown where there is no value; empty for nothing. */
    placeholder: string;
    startOpen?: boolean;
    onChange: (value: string) => void;
    /** Puts a text on the clipboard; defaults to the browser's clipboard. */
    copyText?: (text: string) => void | Promise<void>;
    /** Where many values share one Tab stop: the stop of the link and the buttons (types.ts). */
    innerTabindex?: 0 | -1;
  }

  let {
    name,
    value,
    editable,
    placeholder,
    startOpen = false,
    onChange,
    copyText = (text) => navigator.clipboard.writeText(text),
    innerTabindex,
  }: Props = $props();

  /** What the link and the buttons carry where their Tab stop is given. */
  const inner = $derived(
    innerTabindex === undefined ? {} : { 'data-inner': '', tabindex: innerTabindex },
  );

  // svelte-ignore state_referenced_locally (only where editing starts)
  let editing = $state(startOpen);
  let copied = $state(false);
  let editButton = $state<HTMLButtonElement | null>(null);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  const address = $derived(value.trim());
  const href = $derived(address === '' ? '' : ensureProtocol(address));
  const showField = $derived(editable && (editing || address === ''));

  async function copy(event: MouseEvent): Promise<void> {
    event.stopPropagation();
    try {
      await copyText(value);
    } catch {
      // The browser refused (no permission, not a secure page): say nothing was copied by saying nothing.
      return;
    }
    copied = true;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
      copied = false;
    }, 2000);
  }

  async function done(by: 'enter' | 'escape' | 'blur'): Promise<void> {
    editing = false;
    if (by === 'blur') return;
    // The field is gone; a keyboard user continues from the button that opened it.
    await tick();
    editButton?.focus();
  }

  onDestroy(() => {
    clearTimeout(copiedTimer);
  });
</script>

{#if showField}
  <TextField
    label={name}
    {value}
    {placeholder}
    validate={isUrlEntry}
    errorText={m.property_invalid_url()}
    inputmode="url"
    autofocus={editing}
    onSave={onChange}
    onDone={(by: 'enter' | 'escape' | 'blur') => void done(by)}
  />
{:else if address !== ''}
  <span data-slot="property-url" class="group/url flex min-w-0 items-center gap-0.5">
    {#if isAllowedHref(href)}
      <a
        {...inner}
        {href}
        target="_blank"
        rel="noopener noreferrer"
        class="min-w-0 truncate rounded-sm px-2 py-1.5 text-foreground underline underline-offset-4 hover:text-primary"
        title={value}
        onclick={(event) => {
          event.stopPropagation();
        }}>{value}</a
      >
    {:else}
      <span class="min-w-0 truncate px-2 py-1.5" title={value}>{value}</span>
    {/if}
    <span
      class="flex shrink-0 items-center can-hover:opacity-0 can-hover:group-focus-within/url:opacity-100 can-hover:group-hover/url:opacity-100"
    >
      {#if editable}
        <Button
          {...inner}
          bind:ref={editButton}
          variant="ghost"
          size="icon-sm"
          class="roomy:size-touch"
          aria-label="{name}: {m['URLProperty.edit']()}"
          title={m['URLProperty.edit']()}
          onclick={(event: MouseEvent) => {
            event.stopPropagation();
            editing = true;
          }}
        >
          <PencilIcon aria-hidden="true" />
        </Button>
      {/if}
      <Button
        {...inner}
        variant="ghost"
        size="icon-sm"
        class="roomy:size-touch"
        aria-label="{name}: {copied ? m['URLProperty.copiedLink']() : m['URLProperty.copy']()}"
        title={copied ? m['URLProperty.copiedLink']() : m['URLProperty.copy']()}
        onclick={(event: MouseEvent) => void copy(event)}
      >
        {#if copied}
          <CheckIcon aria-hidden="true" />
        {:else}
          <CopyIcon aria-hidden="true" />
        {/if}
      </Button>
    </span>
    <span class="sr-only" role="status">{copied ? m['URLProperty.copiedLink']() : ''}</span>
  </span>
{:else if placeholder !== ''}
  <span data-slot="property-empty" class="block px-2 py-1.5 text-muted-foreground"
    >{placeholder}</span
  >
{/if}
