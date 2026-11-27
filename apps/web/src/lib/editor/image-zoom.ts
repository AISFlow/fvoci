/**
 * The images of a body that open larger (ImageLightbox.svelte): where a body
 * is only shown, a press of one, or Enter or Space on it, opens it; each is
 * then a button by its description. In the editor a selected image offers a
 * button for it (SelectionBubbleBar.svelte), and where the editor is
 * read-only a press opens it as here.
 *
 * What opens is the address the body drew the image from, and nothing else.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

/** An image to show larger: its address, and its description (empty when it has none). */
export interface LightboxImage {
  src: string;
  alt: string;
}

/** An image of the body that has an address to show. */
const SHOWN_IMAGE = 'img[data-file-id][src]';

/** The image an event is on, if it is one of the body's with an address. */
function imageOf(target: EventTarget | null): HTMLImageElement | undefined {
  if (!(target instanceof Element)) return undefined;
  const image = target.closest(SHOWN_IMAGE);
  return image instanceof HTMLImageElement ? image : undefined;
}

/** What opens of an image: the address it was drawn from, as written. */
export function lightboxOf(image: HTMLImageElement): LightboxImage {
  return { src: image.getAttribute('src') ?? '', alt: image.alt };
}

/** Makes each shown image of `root` a button that opens it, by its description. */
function markImages(root: HTMLElement): void {
  for (const image of root.querySelectorAll<HTMLImageElement>(SHOWN_IMAGE)) {
    image.tabIndex = 0;
    image.setAttribute('role', 'button');
    image.setAttribute('aria-haspopup', 'dialog');
    image.setAttribute(
      'aria-label',
      image.alt === '' ? m.editor_image_zoom() : m.editor_image_zoom_named({ name: image.alt }),
    );
    image.dataset.zoomable = '';
  }
}

interface ZoomOptions {
  /** A body only shown, whose elements are its own: each image is made a button, and its keys open it. */
  buttons: boolean;
  /** Whether a press opens the image now (an editor, while it is read-only); always unless given. */
  when?: () => boolean;
}

/** Opens the images of `root` with `open` as `options` say. Gives back what stops it. */
export function zoomImagesOf(
  root: HTMLElement,
  open: (image: LightboxImage) => void,
  { buttons, when = () => true }: ZoomOptions,
): () => void {
  if (buttons) markImages(root);
  const pressed = (event: MouseEvent): void => {
    const image = imageOf(event.target);
    if (!image || event.button !== 0 || !when()) return;
    // The image's press, and nothing else's: not one that joins a board to write (CardBody.svelte).
    event.preventDefault();
    event.stopPropagation();
    open(lightboxOf(image));
  };
  const keyed = (event: KeyboardEvent): void => {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    const image = imageOf(event.target);
    if (!image || event.target !== image) return;
    event.preventDefault();
    open(lightboxOf(image));
  };
  root.addEventListener('click', pressed);
  if (buttons) root.addEventListener('keydown', keyed);
  return () => {
    root.removeEventListener('click', pressed);
    root.removeEventListener('keydown', keyed);
  };
}
