<!--
  An image of a body as large as the screen lets it be, from the same
  address the body draws it from (image-zoom.ts): no zooming, no dragging.
  Escape, a press beside the image or the close button closes it.

  A modal dialog of the page's own: it stands above whatever the body is in,
  a card's dialog among them, keeps the focus while it is open, and keeps
  its Escape from closing what is under it too.
-->
<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';

  import { m } from '$lib/i18n/paraglide/messages.js';

  import type { LightboxImage } from './image-zoom.ts';

  interface Props {
    /** The image shown; nothing while closed. */
    image: LightboxImage | undefined;
    onclose: () => void;
  }

  let { image, onclose }: Props = $props();

  /**
   * Opens the dialog as it is put in the page, and closes it as it is taken
   * out. The dialog itself takes the focus, so that a press does not light
   * the close button up; Tab reaches it.
   */
  function shown(dialog: HTMLDialogElement): () => void {
    const before = dialog.ownerDocument.activeElement;
    // A page that has no modal dialogs (a simulated one) shows it in place.
    if (typeof dialog.showModal === 'function') dialog.showModal();
    else dialog.setAttribute('open', '');
    dialog.focus();
    return () => {
      if (typeof dialog.close === 'function' && dialog.open) dialog.close();
      // Back where it was: a modal dialog does it itself, a dialog shown in place does not.
      if (before instanceof HTMLElement && before.isConnected)
        before.focus({ preventScroll: true });
    };
  }

  function keys(event: KeyboardEvent): void {
    if (event.key !== 'Escape') return;
    // Not for the dialog of a card under it, which would close too.
    event.preventDefault();
    event.stopPropagation();
    onclose();
  }
</script>

{#if image}
  <dialog
    class="doc-lightbox"
    aria-label={image.alt === '' ? m.editor_image_dialog() : image.alt}
    data-testid="image-lightbox"
    tabindex="-1"
    {@attach shown}
    onkeydown={keys}
    oncancel={(event) => {
      event.preventDefault();
      onclose();
    }}
    onclick={(event) => {
      // Beside the image: on the dialog itself.
      if (event.target === event.currentTarget) onclose();
    }}
  >
    <img src={image.src} alt={image.alt} />
    <button type="button" class="doc-lightbox-close" aria-label={m.ui_close()} onclick={onclose}>
      <XIcon aria-hidden="true" />
    </button>
  </dialog>
{/if}

<style>
  .doc-lightbox {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    width: 100%;
    max-width: none;
    height: 100%;
    max-height: none;
    margin: 0;
    padding: calc(3.5rem + var(--safe-top, 0px)) calc(1rem + var(--safe-right, 0px))
      calc(1.5rem + var(--safe-bottom, 0px)) calc(1rem + var(--safe-left, 0px));
    border: 0;
    background: color-mix(in oklab, var(--background) 94%, transparent);
    color: var(--foreground);
    cursor: zoom-out;
  }
  .doc-lightbox:focus {
    outline: none;
  }
  .doc-lightbox::backdrop {
    background: transparent;
  }
  .doc-lightbox img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: var(--radius-md);
    box-shadow: 0 0.5rem 2rem color-mix(in oklab, var(--foreground) 18%, transparent);
    cursor: default;
  }
  .doc-lightbox-close {
    position: absolute;
    top: calc(0.75rem + var(--safe-top, 0px));
    right: calc(0.75rem + var(--safe-right, 0px));
    display: grid;
    place-items: center;
    width: 2.75rem;
    height: 2.75rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--popover);
    color: var(--popover-foreground);
    cursor: pointer;
  }
  .doc-lightbox-close:hover {
    background: var(--accent);
    color: var(--accent-foreground);
  }
  .doc-lightbox-close:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 2px;
  }
  .doc-lightbox-close :global(svg) {
    width: 1.25rem;
    height: 1.25rem;
  }
</style>
