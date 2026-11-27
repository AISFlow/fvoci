<!--
  A one-line value edited in place. The field is always there; focusing it
  starts editing. Enter and leaving the field save, Escape puts the stored
  value back. `onSave` is called only when the text is acceptable and differs
  from the stored value.

  A text that is not acceptable is not saved: Enter keeps the person in the
  field, and leaving the field keeps the text there, marked as an error, so
  that nothing typed is lost.
-->
<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import type { HTMLInputAttributes } from 'svelte/elements';

  import { isComposing } from '$lib/keys.ts';
  import { cn } from '$lib/utils.js';

  interface Props {
    /** Names the field: the name of the property. */
    label: string;
    value: string;
    placeholder?: string;
    validate?: (text: string) => boolean;
    /** Said when the text is not acceptable. */
    errorText?: string;
    inputmode?: HTMLInputAttributes['inputmode'];
    spellcheck?: boolean;
    /** Focus the field as soon as it is there. */
    autofocus?: boolean;
    class?: string;
    onSave: (text: string) => void;
    /** Editing ended, and how. */
    onDone?: (by: 'enter' | 'escape' | 'blur') => void;
  }

  let {
    label,
    value,
    placeholder,
    validate = () => true,
    errorText,
    inputmode,
    spellcheck = false,
    autofocus = false,
    class: className,
    onSave,
    onDone,
  }: Props = $props();

  const uid = $props.id();
  let input = $state<HTMLInputElement | null>(null);
  let draft = $state(untrack(() => value));
  let editing = $state(false);
  /** Set while the field is being left by Enter or Escape, which have already dealt with the text. */
  let settled = false;
  /**
   * `editing`, for the moment the field is removed. Not reactive on purpose:
   * while a component is being removed its state reads as it was before the
   * change that removes it, and Enter both ends the editing and can remove
   * the field.
   */
  let unsettled = false;

  const invalid = $derived(!validate(draft));

  // A value that changes from outside replaces the text, unless the person is typing.
  $effect(() => {
    const stored = value;
    if (!untrack(() => editing)) draft = stored;
  });

  function save(): boolean {
    if (!validate(draft)) return false;
    if (draft !== value) onSave(draft);
    return true;
  }

  function leave(by: 'enter' | 'escape'): void {
    settled = true;
    input?.blur();
    settled = false;
    editing = false;
    unsettled = false;
    onDone?.(by);
  }

  function onkeydown(event: KeyboardEvent): void {
    // While an input method is composing (Hangul, kana), Enter and Escape are its own.
    if (isComposing(event)) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      if (save()) leave('enter');
    } else if (event.key === 'Escape') {
      event.preventDefault();
      // Escape belongs to the field: it must not also close what the field is in.
      event.stopPropagation();
      draft = value;
      leave('escape');
    }
  }

  function onblur(): void {
    if (settled) return;
    editing = false;
    unsettled = false;
    save();
    onDone?.('blur');
  }

  onMount(() => {
    if (autofocus) input?.focus();
  });

  // Removed while being edited (the card closed, the row scrolled away): keep what was typed.
  onDestroy(() => {
    if (unsettled) save();
  });
</script>

<input
  bind:this={input}
  bind:value={draft}
  type="text"
  data-slot="property-text-field"
  aria-label={label}
  aria-invalid={invalid ? 'true' : undefined}
  aria-describedby={invalid && errorText ? `${uid}-error` : undefined}
  title={invalid && errorText ? errorText : draft}
  autocomplete="off"
  {placeholder}
  {inputmode}
  {spellcheck}
  onfocus={() => {
    editing = true;
    unsettled = true;
  }}
  {onkeydown}
  {onblur}
  class={cn(
    'h-8 w-full min-w-0 rounded-md border border-transparent bg-transparent px-2 text-base text-foreground outline-none placeholder:text-muted-foreground hover:bg-muted focus-visible:border-ring focus-visible:bg-background focus-visible:ring-3 focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 md:text-sm roomy:min-h-touch',
    className,
  )}
/>
{#if invalid && errorText}
  <span id="{uid}-error" class="sr-only" role="alert">{errorText}</span>
{/if}
