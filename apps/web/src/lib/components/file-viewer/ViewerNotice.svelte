<!--
  What the viewer (FileViewer.svelte) shows in the document's place until it
  is shown: that the file opens; a file not drawn in its time, and whether to
  wait on for it or download it instead; or why the file cannot be shown, and
  the file offered as a download. Nothing once it is shown.
-->
<script lang="ts">
  import DownloadIcon from '@lucide/svelte/icons/download';

  import { LIMITS, type Status } from '$lib/file-viewer/session.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { humanFileSize } from '$lib/logic/format.ts';

  interface Props {
    status: Status;
    onDownload: () => void;
    /** The reader's answer for a file not drawn in its time: to wait on for it, or not. */
    onAnswer: (wait: boolean) => void;
  }

  let { status, onDownload, onAnswer }: Props = $props();

  /** The document's place: it keeps the focus while what it says changes under it. */
  let place: HTMLElement | undefined = $state();

  /** What the viewer says when it cannot show the file. */
  const FAILURE: Record<Extract<Status, { name: 'failed' }>['reason'], () => string> = {
    unread: m.viewer_unread,
    unreadable: m.viewer_unreadable,
    locked: m.viewer_locked,
    'too-large': m.viewer_unpacks_too_large,
    slow: m.viewer_slow,
    unstarted: m.viewer_unstarted,
    engine: m.viewer_engine,
    renderer: m.viewer_failed,
  };

  const late = $derived(status.name === 'opening' && status.late);

  /** What the viewer says instead of the document, if anything but that it opens. */
  const notice = $derived.by((): string | undefined => {
    if (status.name === 'too-large')
      return status.what === 'file'
        ? m.viewer_too_large({ size: humanFileSize(LIMITS.bytes) })
        : m.viewer_unpacks_too_large();
    if (status.name === 'unknown') return m.viewer_unknown();
    if (status.name === 'failed') return FAILURE[status.reason]();
    return late ? m.viewer_late() : undefined;
  });

  function answer(wait: boolean): void {
    // Waiting on, the buttons go with the question: the focus stays in the place they were in.
    // Not waiting, the file is given up and offered as a download: that button stays.
    if (wait) place?.focus();
    onAnswer(wait);
    if (!wait) onDownload();
  }
</script>

{#if status.name !== 'shown'}
  <div
    bind:this={place}
    tabindex="-1"
    class="absolute inset-0 flex flex-col items-center justify-center gap-3 p-6 text-center outline-none"
    role="status"
    data-testid={notice ? 'file-viewer-notice' : 'file-viewer-opening'}
  >
    {#if notice}
      <p class="font-medium">{notice}</p>
      <p class="text-muted-foreground">
        {late ? m.viewer_late_choice() : m.viewer_download_instead()}
      </p>
      <div class="flex flex-wrap justify-center gap-2">
        {#if late}
          <button
            type="button"
            class="inline-flex min-h-9 items-center rounded-md border bg-background px-3 hover:bg-accent roomy:min-h-touch"
            onclick={() => {
              answer(true);
            }}
          >
            {m.viewer_wait_on()}
          </button>
        {/if}
        <button
          type="button"
          class="inline-flex min-h-9 items-center gap-2 rounded-md border bg-background px-3 hover:bg-accent roomy:min-h-touch"
          onclick={() => {
            if (late) answer(false);
            else onDownload();
          }}
        >
          <DownloadIcon class="size-4" aria-hidden="true" />
          {m.viewer_download()}
        </button>
      </div>
    {:else}
      <p class="text-muted-foreground">{m.viewer_opening()}</p>
    {/if}
  </div>
{/if}
