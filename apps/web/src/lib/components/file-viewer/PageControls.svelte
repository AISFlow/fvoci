<!--
  The viewer's controls under the document (FileViewer.svelte), within reach
  of a thumb: the previous and the next page (sheet, slide), where the reader
  is among them, and the zoom.
-->
<script lang="ts">
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import ZoomInIcon from '@lucide/svelte/icons/zoom-in';
  import ZoomOutIcon from '@lucide/svelte/icons/zoom-out';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import type { ViewerSession } from '$lib/file-viewer/session.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';

  interface Props {
    session: ViewerSession;
    /** Where the reader is, in words: "page 2 of 7". */
    where: string;
    /** What the buttons to the previous and the next are called. */
    previous: string;
    next: string;
  }

  let { session, where, previous, next }: Props = $props();
</script>

<footer class="flex min-h-12 items-center justify-center gap-1 border-t p-1">
  <IconButton
    label={previous}
    disabled={!session.canTurn(-1)}
    onclick={() => {
      session.turn(-1);
    }}
  >
    <ChevronLeftIcon aria-hidden="true" />
  </IconButton>
  <p class="min-w-16 text-center text-sm tabular-nums" data-testid="file-viewer-page">
    <span aria-hidden="true">{session.page} / {session.pages}</span>
    <span class="sr-only">{where}</span>
  </p>
  <IconButton
    label={next}
    disabled={!session.canTurn(1)}
    onclick={() => {
      session.turn(1);
    }}
  >
    <ChevronRightIcon aria-hidden="true" />
  </IconButton>
  <span class="mx-2 h-6 w-px bg-border" aria-hidden="true"></span>
  <IconButton
    label={m.viewer_zoom_out()}
    disabled={!session.canZoomOut}
    onclick={() => {
      session.zoomBy(-1);
    }}
  >
    <ZoomOutIcon aria-hidden="true" />
  </IconButton>
  <button
    type="button"
    class="min-h-8 min-w-14 rounded-md text-sm tabular-nums hover:bg-accent roomy:min-h-touch"
    aria-label={m.viewer_zoom_reset({ percent: Math.round(session.zoom * 100) })}
    data-testid="file-viewer-zoom"
    onclick={() => {
      session.zoomBy(0);
    }}
  >
    {Math.round(session.zoom * 100)}%
  </button>
  <IconButton
    label={m.viewer_zoom_in()}
    disabled={!session.canZoomIn}
    onclick={() => {
      session.zoomBy(1);
    }}
  >
    <ZoomInIcon aria-hidden="true" />
  </IconButton>
</footer>
