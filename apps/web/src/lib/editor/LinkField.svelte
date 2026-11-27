<!--
  The address of a link, asked for where the tools of chosen text are: the
  bubble over it (SelectionBubbleBar.svelte) and the bar over a phone's
  keyboard (MobileEditorToolbar.svelte). It opens with the address of the
  link already there, chosen to be typed over. Enter puts the link on (an
  empty address takes it off), Escape goes back; the editor has the focus
  again after either. An address a link may not be is refused, and said so
  (link-address.ts).
-->
<script lang="ts">
  import type { Editor } from '@tiptap/core';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isComposing } from '$lib/keys.ts';

  import { linkAt, putLink } from './link-address.ts';

  interface Props {
    editor: Editor;
    /** The field is done with: the link put on or taken off, or left as it was. */
    ondone: () => void;
  }

  let { editor, ondone }: Props = $props();

  let address = $state('');
  let refused = $state(false);

  /** Takes the focus with the link's address in it; reads no state, so it runs once. */
  function opened(field: HTMLInputElement): void {
    const start = linkAt(editor);
    address = start;
    field.value = start;
    field.focus();
    field.select();
  }

  /**
   * Gives the editor the focus now, before the field goes: one taken out with the focus in it
   * leaves the focus nowhere, and tells the bar it is in nothing (no `focusout`).
   */
  function done(): void {
    editor.view.focus();
    ondone();
  }

  function keys(event: KeyboardEvent): void {
    // The Enter that ends a syllable is the input method's (Safari marks it only by its key code).
    if (isComposing(event)) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      refused = !putLink(editor, address);
      if (!refused) done();
    } else if (event.key === 'Escape') {
      // Not for the dialog of a card it is in, which would close.
      event.preventDefault();
      event.stopPropagation();
      done();
    }
  }
</script>

<input
  class="doc-link-field"
  type="text"
  inputmode="url"
  autocomplete="url"
  aria-label={m.editor_format_link_address()}
  aria-invalid={refused || undefined}
  placeholder="https://"
  bind:value={address}
  onkeydown={keys}
  {@attach opened}
/>
{#if refused}
  <p class="doc-link-problem" role="alert">{m.editor_format_link_refused()}</p>
{/if}

<style>
  .doc-link-field {
    width: min(18rem, calc(100vw - 3rem));
    height: 2rem;
    padding: 0 0.5rem;
    border: 1px solid var(--input-border);
    border-radius: var(--radius-md);
    background: var(--background);
    color: var(--foreground);
    /* No smaller than this, or a phone zooms in when the field takes the focus. */
    font-size: var(--text-base);
  }
  .doc-link-field:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 1px;
  }
  .doc-link-problem {
    flex-basis: 100%;
    margin: 0.25rem 0.25rem 0;
    color: var(--destructive);
  }
</style>
