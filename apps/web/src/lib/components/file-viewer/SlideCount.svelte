<!--
  The line under the viewer's header (FileViewer.svelte) for a deck with
  slides hidden from a show: how many slides are laid out, and how many of
  them are hidden, with a button that shows the hidden ones in place or
  leaves them out again.
-->
<script lang="ts">
  import type { ViewerSession } from '$lib/file-viewer/session.svelte.ts';
  import type { SlideOrder } from '$lib/file-viewer/slide-order.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';

  interface Props {
    session: ViewerSession;
    /** The deck's slides. */
    order: SlideOrder;
  }

  let { session, order }: Props = $props();

  const said = $derived.by((): string => {
    const count = order.count(session.showHidden);
    if (order.allHidden) return m.viewer_slides_all_hidden({ count });
    return session.showHidden
      ? m.viewer_slides_count_with_hidden({ count, hidden: order.hidden.size })
      : m.viewer_slides_count({ count });
  });
</script>

<div
  class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b px-4 py-1 text-sm"
  data-testid="file-viewer-slides"
>
  <p class="text-muted-foreground" role="status">{said}</p>
  {#if !order.allHidden}
    <button
      type="button"
      class="min-h-8 rounded-md px-2 font-medium hover:bg-accent roomy:min-h-touch"
      data-testid="file-viewer-hidden"
      onclick={() => {
        session.toggleHidden();
      }}
    >
      {session.showHidden
        ? m.viewer_hide_hidden()
        : m.viewer_show_hidden({ count: order.hidden.size })}
    </button>
  {/if}
</div>
