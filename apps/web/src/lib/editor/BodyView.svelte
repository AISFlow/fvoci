<!--
  A document body for reading: a card preview, a comment, a shared board.

  A thin shell around `renderBody` (render.ts), which does the drawing without
  the editor. Everything comes in as values and callbacks. A body that is not
  a valid document is not drawn at all; `invalid` says what to show in its
  place, and `empty` what to show for a body with nothing in it.

  `renderBody` only makes room for formulas and diagrams; once the body is in
  the page, `drawEmbeds` (embeds.ts) draws them. The libraries for that are
  fetched only when a body has a formula or a diagram. So are the bar, the
  colours and the line numbers of a code block (code-later.ts).

  A reference is an element that holds an identifier; what it names is drawn
  into it from `references` (reference-chip.ts), and again whenever what the
  caller knows of it changes. Without `references` each shows as what the
  reader may not see. A file block is a place too, and its file's name, size
  and link are drawn into it from the file source (file-view.ts). So is a
  board view, into which `boardViews` draws its view; without it (a preview,
  a version, a reader by a link) each says that its view cannot be seen.

  An image opens larger on a press, or on Enter or Space (image-zoom.ts,
  ImageLightbox.svelte), unless `zoomImages` is false: where a press of the
  body is the caller's (a preview that opens what it shows).
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { ClassValue } from 'svelte/elements';

  import './editor.css';
  import { CODE_COPY_TEXTS } from './code-copy-texts.ts';
  import { drawCodeBlocksLater } from './code-later.ts';
  import { mountBoardViews, type BoardViewHost } from './board-view-read.ts';
  import './board-view.css';
  import { drawEmbeds } from './embeds.ts';
  import { drawFileBlocks } from './file-view.ts';
  import ImageLightbox from './ImageLightbox.svelte';
  import { zoomImagesOf, type LightboxImage } from './image-zoom.ts';
  import { attachReferences, type ReferenceDrawing } from './reference-chip.ts';
  import { renderBody } from './render.ts';

  interface Props {
    /** The stored document, parsed from JSON. `null` or nothing is an empty body. */
    doc: unknown;
    /** The name of a mentioned user; the identifier is shown when there is none. */
    resolveUser?: (userId: string) => string | undefined;
    /** The accessible name of the checkbox of a task, from the text of the task. */
    taskLabel?: (text: string, checked: boolean) => string;
    /** What the references of the body are drawn from, and how one is opened. */
    references?: ReferenceDrawing;
    /** What draws the views of the board views of a card's, a page's or a unit's body. */
    boardViews?: BoardViewHost | undefined;
    /** Shown in place of a body that cannot be shown; gets what is wrong with it. */
    invalid?: Snippet<[problems: string[]]>;
    /** Shown in place of an empty body. */
    empty?: Snippet;
    /** Whether an image opens larger when pressed; true unless given. */
    zoomImages?: boolean;
    class?: ClassValue;
  }

  let {
    doc,
    resolveUser,
    taskLabel,
    references,
    boardViews,
    invalid,
    empty,
    zoomImages = true,
    class: className,
  }: Props = $props();

  /** The image opened larger; nothing while none is. */
  let zoomed = $state<LightboxImage>();

  /** The images of the body open larger, those it draws anew too (`nodes`). */
  function zoomImagesIn(
    element: HTMLElement,
    zooms: boolean,
    ..._drawn: unknown[]
  ): (() => void) | undefined {
    if (!zooms) return undefined;
    return zoomImagesOf(element, (image) => (zoomed = image), { buttons: true });
  }

  const body = $derived.by(() => {
    const rendered = renderBody(doc, { resolveUser, taskLabel });
    // A fragment gives its nodes away once; the list can be placed again.
    return rendered.ok ? { ...rendered, nodes: [...rendered.fragment.childNodes] } : rendered;
  });
</script>

{#if !body.ok}
  {@render invalid?.(body.problems)}
{:else if body.empty}
  {@render empty?.()}
{:else}
  {@const nodes = body.nodes}
  <div
    class={['doc-body', className]}
    {@attach (element: HTMLElement) => {
      element.replaceChildren(...nodes);
      // Each file block's name, size and link, drawn as in the editor (file-view.ts).
      drawFileBlocks(element);
      // Gives back what stops the drawing when the body changes or goes away.
      return drawEmbeds(element);
    }}
    {@attach (element: HTMLElement) => attachReferences(element, references, nodes)}
    {@attach (element: HTMLElement) => mountBoardViews(element, boardViews?.mount, nodes)}
    {@attach (element: HTMLElement) => drawCodeBlocksLater(element, CODE_COPY_TEXTS, nodes)}
    {@attach (element: HTMLElement) => zoomImagesIn(element, zoomImages, nodes)}
  ></div>
  <ImageLightbox image={zoomed} onclose={() => (zoomed = undefined)} />
{/if}
