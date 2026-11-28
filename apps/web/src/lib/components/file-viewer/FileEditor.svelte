<!--
  A Hangul (HWP, HWPX) document in the editor, in the viewer's place
  (FileViewerHost.svelte): rhwp-studio in a frame that has no origin and no
  network (lib/file-viewer/editing.svelte.ts, src/editor-frame/). The header
  says whether there are changes that are not saved, and saves them: "Save"
  here, or Ctrl+S in the document.

  A save makes the document the file's new version. When rhwp says the saved
  file loses something of the document, nothing is saved until the writer
  has seen what and chosen: a new version all the same, a file of its own
  beside this one, or neither. When the file got a newer version than the
  one edited, it is not changed, and the writer may keep the edit as a file
  of its own.

  Closing with changes that are not saved asks first, and so does the
  browser before the page goes. The file's lease (lib/file-viewer/lease.svelte.ts)
  is held on to while the editor is open and the writer at work: with no input
  for 30 minutes it is let go of, which the header says (and, 5 minutes before,
  that it will be), and input takes it again. Where somebody else took it
  meanwhile, the header says so. It is mounted for one version and closed by
  unmounting it, which takes the frame out of the page.
-->
<script lang="ts">
  import SaveIcon from '@lucide/svelte/icons/save';
  import XIcon from '@lucide/svelte/icons/x';
  import { onDestroy, untrack } from 'svelte';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { display } from '$lib/display.svelte.ts';
  import {
    EDIT_LIMITS,
    editorAddress,
    EditingSession,
    markedName,
    type EditedFile,
    type EditStatus,
    type SaveFailure,
  } from '$lib/file-viewer/editing.svelte.ts';
  import { IDLE_AFTER, type EditLease } from '$lib/file-viewer/lease.svelte.ts';
  import type { VersionsSource } from '$lib/file-viewer/versions.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import { page } from '$lib/layout/room.svelte.ts';
  import { humanFileSize } from '$lib/logic/format.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';

  import SaveChoice from './SaveChoice.svelte';

  interface Props {
    /** The version edited; the editor is made for one and is not handed another. */
    file: EditedFile;
    /** The file's versions: where a save goes. */
    versions: VersionsSource;
    /** The file's lease, held while the editor is open. */
    lease?: EditLease | undefined;
    onClose: () => void;
  }

  let { file, versions, lease, onClose }: Props = $props();

  /** The name a file of its own is kept under: the edited file's, marked as an edit. */
  function apartName(name: string): string {
    return markedName(name, (stem) => m.file_editor_apart_name({ stem }));
  }

  /** The header: where the focus goes when the writer asks out of the document (F6). */
  let header: HTMLElement | undefined;
  function asHeader(node: HTMLElement): () => void {
    header = node;
    return () => {
      header = undefined;
    };
  }
  const session = untrack(
    () =>
      new EditingSession(file, versions, {
        theme: () => display.resolved.theme,
        lang: getLocale,
        apartName,
        // Out of the document, which keeps Tab and Escape to itself: to the first control.
        onLeave: () => header?.querySelector<HTMLElement>('button:not(:disabled)')?.focus(),
        onInput: () => lease?.touch(),
      }),
  );
  onDestroy(() => {
    session.close();
  });

  const address = editorAddress(document);
  const compact = $derived(page.stage === 'compact');
  /** Short of the widest screens, what the editor says has a line of its own, whole. */
  // What the lease says is long: a line of its own, whole, on the widest screens too.
  const sayApart = $derived(
    page.stage !== 'expanded' ||
      lease?.lost === true ||
      lease?.resting === true ||
      lease?.idleSoon === true,
  );
  const status = $derived(session.status);
  const saving = $derived(session.saving);
  /** Asked whether to close over changes that are not saved. */
  let discarding = $state(false);

  const FAILURE: Record<Extract<EditStatus, { name: 'failed' }>['reason'], () => string> = {
    unreadable: m.file_editor_failed,
    editor: m.file_editor_failed,
    locked: m.file_editor_locked,
    'too-large': m.file_editor_too_large,
    slow: m.file_editor_slow,
    unstarted: m.file_editor_unstarted,
    engine: m.file_editor_engine,
  };

  const SAVE_FAILURE: Record<SaveFailure, () => string> = {
    export: m.file_editor_save_export,
    'not-a-document': m.file_editor_save_not_document,
    'too-large': () => m.file_editor_save_too_large({ size: humanFileSize(EDIT_LIMITS.bytes) }),
    upload: m.file_editor_save_upload,
    stopped: m.file_editor_save_stopped,
    refused: m.file_editor_save_refused,
  };

  /**
   * What the header says of the document: a save under way, the lease let go of for want of
   * input (or soon to be), how a save ended, somebody else editing, changes not saved. A
   * save's end said 25 minutes ago or more gives way to the lease: it is said where the lease
   * goes, as somebody else editing is where they took it.
   */
  const said = $derived.by((): string => {
    if (saving.name === 'exporting' || saving.name === 'storing') return m.file_editor_saving();
    if (lease?.resting || lease?.idleSoon) {
      if (lease.lost) return m.file_editor_lease_lost();
      return lease.resting
        ? m.file_editor_lease_resting({ minutes: IDLE_AFTER / 60_000 })
        : m.file_editor_lease_idle_soon({ minutes: lease.minutesLeft });
    }
    if (saving.name === 'failed') return SAVE_FAILURE[saving.reason]();
    if (lease?.lost) return m.file_editor_lease_lost();
    if (session.dirty) return m.file_editor_unsaved();
    const saved = session.saved;
    if (saved?.as === 'version') {
      return m.file_editor_saved_version({ version: saved.version.version });
    }
    return saved ? m.file_editor_saved_apart({ name: saved.name }) : '';
  });

  /** Input in the app's own controls: before they answer it, which may stop it going on. */
  const touch = (): void => {
    lease?.touch();
  };

  function saveOnKey(event: KeyboardEvent): void {
    if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) return;
    if (event.key.toLowerCase() !== 's' && event.key !== 'ㄴ') return;
    event.preventDefault();
    session.save();
  }

  /** The page goes with changes that are not saved, or a save under way: the browser asks first. */
  function beforeUnload(event: BeforeUnloadEvent): void {
    if (session.dirty || session.busy) event.preventDefault();
  }

  function close(): void {
    // A save under way, or a choice waiting, is ended first.
    if (session.busy) return;
    if (session.dirty) discarding = true;
    else onClose();
  }
</script>

<!-- Ctrl+S (Cmd+S) saves from the app's own controls too, as it does from the document. -->
<svelte:window
  onkeydown={saveOnKey}
  onkeydowncapture={touch}
  onpointerdowncapture={touch}
  onbeforeunload={beforeUnload}
/>

<!-- Open while it is mounted: Escape and the like ask to close, and `close` decides. -->
<Dialog.Root
  bind:open={
    () => true,
    (open) => {
      if (!open) close();
    }
  }
>
  <Dialog.Content
    showCloseButton={false}
    data-testid="file-editor"
    data-status={status.name}
    data-saving={saving.name}
    data-dirty={session.dirty}
    data-lease={lease?.lost ? 'lost' : lease?.resting ? 'resting' : 'held'}
    class={[
      'flex flex-col gap-0 overflow-hidden bg-background p-0',
      compact
        ? 'top-0 left-0 h-dvh w-full max-w-none translate-x-0 translate-y-0 rounded-none pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none'
        : 'h-[calc(100dvh-2rem)] max-w-[calc(100%-2rem)] sm:max-w-[calc(100%-2rem)]',
    ]}
    interactOutsideBehavior="ignore"
  >
    <header
      {@attach asHeader}
      class={[
        'flex min-h-12 items-center gap-x-2 border-b py-1 pr-1 pl-4',
        sayApart && 'flex-wrap',
      ]}
    >
      <Dialog.Title class="min-w-0 flex-1 truncate text-base font-semibold">
        {file.name}
      </Dialog.Title>
      <Dialog.Description class="sr-only">{m.file_editor_description()}</Dialog.Description>
      <p
        class={[
          'min-w-0 text-sm',
          sayApart ? 'order-last basis-full pr-3' : 'shrink truncate',
          saving.name === 'failed' || lease?.lost ? 'text-destructive' : 'text-muted-foreground',
        ]}
        role="status"
        data-testid="file-editor-said"
      >
        {said}
      </p>
      <Button
        class="shrink-0 roomy:min-h-touch"
        disabled={status.name !== 'ready' || session.busy}
        data-testid="file-editor-save"
        onclick={() => {
          session.save();
        }}
      >
        <SaveIcon data-icon="inline-start" aria-hidden="true" />
        {m.file_editor_save()}
      </Button>
      <IconButton
        label={m.ui_close()}
        disabled={session.busy}
        data-testid="file-editor-close"
        onclick={close}
      >
        <XIcon aria-hidden="true" />
      </IconButton>
    </header>

    <div class="relative min-h-0 flex-1 bg-muted">
      {#if status.name !== 'failed'}
        <iframe
          {@attach session.attach}
          src={address}
          sandbox="allow-scripts"
          referrerpolicy="no-referrer"
          title={m.file_editor_frame_title({ name: file.name })}
          class={['size-full border-0', status.name !== 'ready' && 'invisible']}
          data-testid="file-editor-frame"
        ></iframe>
      {/if}
      {#if status.name === 'opening'}
        <p
          class="absolute inset-0 flex items-center justify-center text-muted-foreground"
          role="status"
        >
          {m.file_editor_opening()}
        </p>
      {:else if status.name === 'failed'}
        <div
          class="absolute inset-0 flex flex-col items-center justify-center gap-3 p-6 text-center"
          role="status"
          data-testid="file-editor-notice"
        >
          <p class="font-medium">{FAILURE[status.reason]()}</p>
          <Button variant="outline" class="roomy:min-h-touch" onclick={onClose}>
            {m.ui_close()}
          </Button>
        </div>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<SaveChoice {session} />

<ConfirmDialog
  bind:open={discarding}
  title={m.file_editor_discard_title()}
  description={m.file_editor_discard_description()}
  confirmLabel={m.file_editor_discard_confirm()}
  destructive
  onConfirm={onClose}
/>
