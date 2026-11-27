<!--
  The files attached to a card: their names and sizes; a file's name opens it
  in the viewer (lib/components/file-viewer, over the card, as a file
  block's does: `viewing.svelte.ts`), which lists its versions
  (file-versions.ts), and a button beside it
  downloads it; and, where the reader may edit, attaching more (the
  button, or files dropped on this part of the card; several at once),
  following an upload while it runs, giving it up, and deleting a file.

  An upload is where every upload is too (the shell's stack, which tells of
  its start, its end and why it failed): here is its place in the list, the
  same upload, dismissed or given up from either. It goes on when the card
  is closed. A deleted attachment can be brought back with "Undo" (the card's
  menu), so deleting does not ask first. An upload that failed stays in the
  list, marked, until the reader dismisses it.
-->
<script lang="ts">
  import DownloadIcon from '@lucide/svelte/icons/download';
  import FileTextIcon from '@lucide/svelte/icons/file-text';
  import PaperclipIcon from '@lucide/svelte/icons/paperclip';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import UploadIcon from '@lucide/svelte/icons/upload';
  import XIcon from '@lucide/svelte/icons/x';

  import { viewing } from '$lib/components/file-viewer/viewing.svelte.ts';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { humanFileSize } from '$lib/logic/format.ts';
  import type { Attachment, BoardStore, CardStore, Transfer } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { saveBlob } from '$lib/save.ts';

  import { attachmentVersions } from './file-versions.ts';

  interface Props {
    board: BoardStore;
    card: CardStore;
  }

  let { board, card }: Props = $props();

  const uid = $props.id();
  const editable = $derived(board.canEdit);

  let input = $state<HTMLInputElement>();
  /** Files are being dragged over this part of the card. */
  let over = $state(false);
  /** How many elements of this part the drag is inside of: it enters a child before it leaves the parent. */
  let inside = 0;

  function attach(files: Iterable<File> | null | undefined): void {
    for (const file of files ?? []) void card.attach(file);
  }

  const carriesFiles = (event: DragEvent): boolean =>
    editable && (event.dataTransfer?.types.includes('Files') ?? false);

  function ondragenter(event: DragEvent): void {
    if (!carriesFiles(event)) return;
    inside += 1;
    over = true;
  }

  function ondragover(event: DragEvent): void {
    // Saying so is what makes this a place to drop on.
    if (carriesFiles(event)) event.preventDefault();
  }

  function ondragleave(event: DragEvent): void {
    if (!carriesFiles(event)) return;
    inside = Math.max(0, inside - 1);
    over = inside > 0;
  }

  function ondrop(event: DragEvent): void {
    if (!carriesFiles(event)) return;
    event.preventDefault();
    inside = 0;
    over = false;
    attach(event.dataTransfer?.files);
  }

  const plainPress = (event: MouseEvent): boolean =>
    event.button === 0 && !event.ctrlKey && !event.metaKey && !event.shiftKey && !event.altKey;

  /**
   * A plain press on a file's name opens it in the viewer; one with a modifier
   * key does what the browser does with a link. Should the viewer not arrive
   * (the connection dropped), the file is saved instead.
   */
  function onView(event: MouseEvent, attachment: Attachment): void {
    if (!plainPress(event)) return;
    event.preventDefault();
    const { id, fileId, name, size } = attachment;
    const file = { name, size, read: (signal?: AbortSignal) => card.fileBytes(fileId, signal) };
    // Closed, the viewer gives the focus back to the name it was opened from.
    const back = () =>
      document.querySelector<HTMLElement>(`[data-attachment="${CSS.escape(id)}"] a`);
    const versions = attachmentVersions(board, card, attachment);
    void viewing.open(file, back, versions).then((arrived) => {
      if (!arrived) download(fileId, name);
    });
  }

  /** Saves a file under its own name (save.ts). */
  function download(fileId: string, name: string): void {
    // A file being read is not read again by another press.
    if (downloading.includes(fileId)) return;
    downloading = [...downloading, fileId];
    void card
      .fileBytes(fileId)
      .then((bytes) => {
        if (bytes) saveBlob(bytes, name);
      })
      .finally(() => {
        downloading = downloading.filter((id) => id !== fileId);
      });
  }

  /** Files whose bytes are being read, to be saved. */
  let downloading = $state<string[]>([]);

  const percent = (upload: Transfer): number =>
    upload.total > 0 ? Math.min(100, Math.round((upload.done / upload.total) * 100)) : 0;
</script>

{#if editable || card.attachments.length > 0}
  <section
    aria-labelledby="{uid}-heading"
    class={[
      'flex flex-col gap-2 border-t pt-5',
      over && 'rounded-lg outline-2 outline-offset-4 outline-ring outline-dashed',
    ]}
    data-testid="card-attachments"
    data-over={over ? '' : undefined}
    {ondragenter}
    {ondragover}
    {ondragleave}
    {ondrop}
  >
    <div class="flex min-h-8 items-center gap-2">
      <h3 id="{uid}-heading" class="min-w-0 flex-1 font-semibold">
        {m.card_attachments()}
        <span class="font-normal text-muted-foreground">{card.attachments.length}</span>
      </h3>
      {#if editable}
        <Button
          variant="outline"
          class="roomy:min-h-touch"
          data-testid="attach"
          onclick={() => input?.click()}
        >
          <PaperclipIcon data-icon="inline-start" aria-hidden="true" />
          {m['CardDetail.Attach']()}
        </Button>
        <input
          bind:this={input}
          type="file"
          multiple
          class="sr-only"
          tabindex="-1"
          aria-hidden="true"
          data-testid="attach-input"
          onchange={(event) => {
            attach(event.currentTarget.files);
            // The same file can be chosen again.
            event.currentTarget.value = '';
          }}
        />
      {/if}
    </div>

    {#if over}
      <p class="text-muted-foreground">{m.card_attachment_drop()}</p>
    {/if}

    {#if card.attachments.length > 0}
      <ul class="flex flex-col gap-2">
        {#each card.attachments as attachment (attachment.id)}
          {@const url = card.fileUrl(attachment.fileId)}
          <li
            class="flex items-center gap-3 rounded-lg border py-1 pr-1 pl-3"
            data-attachment={attachment.id}
          >
            <FileTextIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
            <div class="min-w-0 flex-1 py-0.5">
              {#if url}
                <a
                  href={url}
                  download={attachment.name}
                  onclick={(event) => {
                    onView(event, attachment);
                  }}
                  title={m.card_attachment_view({ name: attachment.name })}
                  class="block truncate underline-offset-2 hover:underline roomy:py-1"
                >
                  {attachment.name}
                </a>
              {:else}
                <p class="truncate">{attachment.name}</p>
              {/if}
              <!-- 0: an attachment another client made may say no size. -->
              {#if attachment.size > 0}
                <p class="text-xs text-muted-foreground">{humanFileSize(attachment.size)}</p>
              {/if}
            </div>
            {#if url}
              <IconButton
                label={m.card_attachment_download({ name: attachment.name })}
                aria-busy={downloading.includes(attachment.fileId)}
                onclick={() => {
                  download(attachment.fileId, attachment.name);
                }}
              >
                <DownloadIcon aria-hidden="true" />
              </IconButton>
            {/if}
            {#if editable}
              <IconButton
                label={m.card_attachment_delete({ name: attachment.name })}
                onclick={() => void card.deleteAttachment(attachment.id)}
              >
                <Trash2Icon aria-hidden="true" />
              </IconButton>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    <ul class="flex flex-col gap-2 empty:hidden" data-testid="card-uploads">
      {#each card.uploads as upload (upload.id)}
        <li
          class="flex items-center gap-3 rounded-lg border border-dashed py-1 pr-1 pl-3"
          data-upload={upload.name}
          data-failed={upload.failure ? '' : undefined}
        >
          <UploadIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
          <div class="min-w-0 flex-1 py-0.5">
            <p class="truncate">{upload.name}</p>
            {#if upload.failure}
              <p class="text-xs text-destructive">{m.card_upload_failed()}</p>
            {:else}
              <div
                role="progressbar"
                aria-label={m.card_upload_progress({ name: upload.name })}
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow={percent(upload)}
                class="mt-1.5 mb-1 h-1.5 overflow-hidden rounded-full bg-muted"
              >
                <div
                  class="h-full rounded-full bg-primary motion-safe:transition-[width]"
                  style:width="{percent(upload)}%"
                ></div>
              </div>
            {/if}
          </div>
          <IconButton
            label={upload.failure
              ? m.card_upload_dismiss({ name: upload.name })
              : m.card_upload_cancel({ name: upload.name })}
            onclick={() => {
              board.workspace.transfers.dismiss(upload.id);
            }}
          >
            <XIcon aria-hidden="true" />
          </IconButton>
        </li>
      {/each}
    </ul>
  </section>
{/if}
