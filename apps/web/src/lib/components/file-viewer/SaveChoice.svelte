<!--
  What the editor (FileEditor.svelte) asks before a save that is not a plain
  new version, over the editor:

  - the saved file loses something of the document (as rhwp's report names
    it, or a part of the package or a stream rhwp does not keep): what, where
    and why, and the choice between a new version all the same, a file of its
    own beside this one, and not saving. A distribution document (배포용
    문서), which the saved file is not, is said first, in the same question,
    and the counts of pages and controls the saved file reads back with
    otherwise than the document in the editor (157 pages → 159) last;
  - the file got a newer version than the one edited: the choice between
    keeping the edit as a file of its own and not saving.

  Not saving leaves the document as it is in the editor, its changes unsaved.
-->
<script lang="ts">
  import { controlName } from '$lib/file-viewer/control-names.ts';
  import type { EditingSession } from '$lib/file-viewer/editing.svelte.ts';
  import type { Loss, Recount } from '$lib/file-viewer/editor-protocol.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import * as AlertDialog from '$lib/ui/alert-dialog/index.js';

  interface Props {
    session: EditingSession;
  }

  let { session }: Props = $props();

  const saving = $derived(session.saving);
  type Lossy = Extract<EditingSession['saving'], { name: 'lossy' }>;
  /** The report asked about; the last one stays while its dialog closes, not to empty as it goes. */
  let last: Lossy | undefined;
  const lossy = $derived.by(() => {
    if (saving.name === 'lossy') last = saving;
    return last;
  });

  /** The question's title: what weighs most of what the save does. */
  const title = $derived.by(() => {
    if (lossy?.unprotected) return m.file_editor_unprotect_title();
    return (lossy?.count ?? 0) > 0 ? m.file_editor_loss_title() : m.file_editor_recount_title();
  });

  /** All the question says, in one description, so that a screen reader reads it with the title. */
  const description = $derived.by(() => {
    if (!lossy) return '';
    const { count } = lossy;
    const said: string[] = [];
    if (lossy.unprotected) said.push(m.file_editor_unprotect_description());
    if (count > 0) {
      said.push(
        lossy.unprotected
          ? m.file_editor_unprotect_losses({ count })
          : m.file_editor_loss_description({ count }),
      );
    }
    // "Also, as listed after them": after the losses' list; else the counts' own description.
    if (lossy.recounts.length > 0) {
      said.push(count > 0 ? m.file_editor_recount_also() : m.file_editor_recount_description());
    }
    return said.join(' ');
  });

  const recounted = (recount: Recount): string =>
    recount.of === 'pages'
      ? m.file_editor_recount_pages(recount)
      : m.file_editor_recount_control({ ...recount, name: controlName(recount.id) });

  const WHAT: Record<Loss['code'], () => string> = {
    binaryContentEmptied: m.file_editor_loss_binary,
    controlOmitted: m.file_editor_loss_control,
    metadataReduced: m.file_editor_loss_metadata,
    partOmitted: m.file_editor_loss_part,
  };
  const WHY: Record<Loss['reason'], () => string> = {
    resourceReadFailedOrLimitExceeded: m.file_editor_loss_reason_read,
    storedCompressionMismatch: m.file_editor_loss_reason_compression,
    rawPassthroughUnavailable: m.file_editor_loss_reason_passthrough,
    unsupportedByOutputFormat: m.file_editor_loss_reason_format,
    notKeptByEditor: m.file_editor_loss_reason_kept,
    previewMadeAgain: m.file_editor_loss_reason_preview,
  };
</script>

<AlertDialog.Root
  open={saving.name === 'lossy'}
  onOpenChange={(open) => {
    if (!open) session.dismiss();
  }}
>
  <!-- Its three choices side by side, from the width they fit at: the default is narrower. -->
  <AlertDialog.Content data-testid="file-editor-losses" class="data-[size=default]:sm:max-w-lg">
    <AlertDialog.Header>
      <AlertDialog.Title>{title}</AlertDialog.Title>
      <AlertDialog.Description>{description}</AlertDialog.Description>
    </AlertDialog.Header>
    {#if lossy && (lossy.count > 0 || lossy.recounts.length > 0)}
      <!-- Focusable, so that the keys scroll it where it is longer than the box (Safari). -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        class="max-h-64 overflow-y-auto rounded-md border text-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        role="region"
        aria-label={m.file_editor_changes_list()}
        tabindex="0"
      >
        {#if lossy.count > 0}
          <ul>
            {#each lossy.losses as loss, at (at)}
              <li class="border-b px-3 py-2 last:border-b-0">
                <p>{WHAT[loss.code]()}: {WHY[loss.reason]()}</p>
                <p class="font-mono text-xs break-all text-muted-foreground">{loss.path}</p>
              </li>
            {/each}
            {#if lossy.count > lossy.losses.length}
              <li class="px-3 py-2 text-muted-foreground">
                {m.file_editor_loss_more({ count: lossy.count - lossy.losses.length })}
              </li>
            {/if}
          </ul>
        {/if}
        {#if lossy.recounts.length > 0}
          <ul data-testid="file-editor-recounts" class={[lossy.count > 0 && 'border-t']}>
            {#each lossy.recounts as recount (recount.of === 'pages' ? '' : `:${recount.id}`)}
              <li class="border-b px-3 py-2 last:border-b-0">{recounted(recount)}</li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
    <AlertDialog.Footer>
      <AlertDialog.Cancel class="roomy:min-h-touch">{m.shared_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="outline"
        class="roomy:min-h-touch"
        data-testid="file-editor-losses-apart"
        onclick={() => {
          session.saveApart();
        }}
      >
        {m.file_version_save_apart()}
      </AlertDialog.Action>
      <AlertDialog.Action
        variant="destructive"
        class="roomy:min-h-touch"
        data-testid="file-editor-losses-version"
        onclick={() => {
          session.saveWithLosses();
        }}
      >
        {m.file_editor_save_as_version()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root
  open={saving.name === 'confirm'}
  onOpenChange={(open) => {
    if (!open) session.dismiss();
  }}
>
  <AlertDialog.Content data-testid="file-editor-confirm" class="data-[size=default]:sm:max-w-lg">
    <AlertDialog.Header>
      <AlertDialog.Title>{m.file_editor_confirm_title()}</AlertDialog.Title>
      <AlertDialog.Description>{m.file_editor_confirm_description()}</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel class="roomy:min-h-touch">{m.shared_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="outline"
        class="roomy:min-h-touch"
        data-testid="file-editor-confirm-apart"
        onclick={() => {
          session.saveApart();
        }}
      >
        {m.file_version_save_apart()}
      </AlertDialog.Action>
      <AlertDialog.Action
        class="roomy:min-h-touch"
        data-testid="file-editor-confirm-version"
        onclick={() => {
          session.confirmSave();
        }}
      >
        {m.file_editor_save_as_version()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root
  open={saving.name === 'stale'}
  onOpenChange={(open) => {
    if (!open) session.dismiss();
  }}
>
  <AlertDialog.Content data-testid="file-editor-stale">
    <AlertDialog.Header>
      <AlertDialog.Title>{m.file_editor_stale_title()}</AlertDialog.Title>
      <AlertDialog.Description>{m.file_editor_stale_description()}</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel class="roomy:min-h-touch">{m.shared_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        class="roomy:min-h-touch"
        data-testid="file-editor-stale-apart"
        onclick={() => {
          session.saveApart();
        }}
      >
        {m.file_version_save_apart()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
