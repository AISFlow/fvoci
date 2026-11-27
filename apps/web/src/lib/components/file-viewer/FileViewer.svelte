<!--
  A file shown in the app instead of downloaded: a PDF, a Word document, a
  workbook, a PowerPoint deck or a Hangul (HWP, HWPX) document, drawn in a
  frame that has no origin and no network (lib/file-viewer/session.svelte.ts,
  src/viewer-frame/). A workbook's pages are its sheets, named in tabs under
  it and in what is announced; a deck's are its slides. A deck with slides
  hidden from a show opens on the others, under a line that counts them and
  a button that shows the hidden ones in place (marked in the frame and in
  what is announced); a slide keeps its number, which jumps where slides
  are left out.

  A dialog over the page on a wide screen, the whole screen on a narrow one.
  Pages and zoom are at the bottom, within reach of a thumb. The keys, while
  the focus is in the viewer or in the document: the left and right arrows
  for the previous and next page, + and − to zoom, 0 for 100%, Escape to
  close. A file that cannot be shown here says why and offers itself as a
  download; one not drawn in its time asks whether to wait on for it
  (ViewerNotice.svelte).

  Where the file has versions to show (`versions`: not for who reads by a
  public link), the file is read as its newest version by that version's
  number, so that the version on screen is known and not guessed. A button in
  the header opens their list (FileVersions.svelte) beside the document, or
  over it on a narrow screen. A version picked there is shown in the file's
  place, in a frame of its own; the header says when what is shown is not the
  newest.

  A Hangul document can be edited by who may write the file's versions: "Edit"
  takes the file's lease (one editor at a time, lib/file-viewer/lease.svelte.ts)
  and hands the version on screen to the editor (FileEditor.svelte), which
  takes the viewer's place until it is closed. Where a newer version was saved
  meanwhile, that one is shown instead, to be edited from there. A line under
  the header says who edits the file; "Edit" stays where it is meanwhile,
  unavailable and described by that line, so that a focus on it stays.

  It is mounted for one file and closed by unmounting it, which takes the
  frame out of the page and stops whatever the frame was doing.
-->
<script lang="ts">
  import DownloadIcon from '@lucide/svelte/icons/download';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import XIcon from '@lucide/svelte/icons/x';
  import { onDestroy, tick, untrack } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';

  import EditingNote from './EditingNote.svelte';
  import PageControls from './PageControls.svelte';
  import SheetTabs from './SheetTabs.svelte';
  import SlideCount from './SlideCount.svelte';
  import ViewerNotice from './ViewerNotice.svelte';
  import { display } from '$lib/display.svelte.ts';
  import type { EditedFile } from '$lib/file-viewer/editing.svelte.ts';
  import { LeaseWatch, startEditing, type EditLease } from '$lib/file-viewer/lease.svelte.ts';
  import { frameAddress, ViewerSession, type ViewedFile } from '$lib/file-viewer/session.svelte.ts';
  import {
    VersionsList,
    type FileVersion,
    type VersionsSource,
  } from '$lib/file-viewer/versions.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import type { Regional } from '$lib/logic/regional.ts';
  import { saveBlob } from '$lib/save.ts';
  import * as Dialog from '$lib/ui/dialog/index.js';

  import FileVersions from './FileVersions.svelte';

  interface Props {
    /** The file; the viewer is made for one and is not handed another. */
    file: ViewedFile;
    /** The file's versions, where the reader may see them. */
    versions?: VersionsSource | undefined;
    regional?: Regional | undefined;
    onClose: () => void;
    /** Opens the editor on the version shown, with the file's lease; without it the viewer offers no editing. */
    onEdit?: ((edited: EditedFile, lease: EditLease) => void) | undefined;
    /** A lease of the reader's being let go of: who edits the file is asked once it went. */
    released?: Promise<void>;
    /**
     * Gives the focus back as the viewer goes, once its hold on the focus is let go of; without
     * it, the focus goes back to what had it when the viewer opened.
     */
    returnFocus?: () => void;
  }

  let { file, versions, regional, onClose, onEdit, released, returnFocus }: Props = $props();

  const uid = $props.id();

  const sessionOf = (viewed: ViewedFile): ViewerSession =>
    new ViewerSession(viewed, {
      theme: () => display.resolved.theme,
      lang: getLocale,
      onKey: (key) => {
        command(key);
      },
      onDownload: () => {
        download();
      },
    });

  /** The list of versions, read as the viewer opens: the file is read by what it says. */
  const list = untrack(() => (versions ? new VersionsList(versions) : undefined));
  /** The number of the version on screen, where it is known: the one read, then whichever was picked. */
  let shownVersion = $state<number>();
  /** What is on screen: the file as it was opened, then whichever version was picked. */
  let session = $state.raw(untrack(() => sessionOf(opening())));
  let listed = $state(false);
  let panel = $state<HTMLElement>();
  /** Who edits the file, asked of a Hangul document whose versions the reader sees. */
  const watch = untrack(() =>
    versions && /\.hwpx?$/i.test(file.name) ? new LeaseWatch(versions) : undefined,
  );
  $effect(() => watch?.watch(untrack(() => released)));
  /** "Edit" is under way: its lease is asked for. */
  let taking = $state(false);
  /** What came of the last "Edit", where it is to be said. */
  let editNote = $state<'failed' | 'newer'>();
  /** The viewer is closed: what ends after shows nothing. */
  let closed = false;
  onDestroy(() => {
    closed = true;
  });

  /**
   * The file as the viewer opens it. Where its versions are listed it is read
   * as the newest of them by number, and that number is the version on
   * screen; where the list could not be read, as the file is, its version
   * unknown.
   */
  function opening(): ViewedFile {
    const source = versions;
    if (!source || !list) return file;
    const { name, size, read } = file;
    let opened: FileVersion | undefined;
    return {
      name,
      size,
      read: async (signal) => {
        await list.first;
        // The same version however often it is read: the download is of what is on screen.
        opened ??= list.current;
        if (!opened) return read(signal);
        const blob = await source.bytes(opened.version, signal);
        // Not over a version picked meanwhile.
        if (blob) shownVersion ??= opened.version;
        return blob;
      },
    };
  }

  /** What is on screen is not the file's newest version. */
  const older = $derived(
    shownVersion !== undefined &&
      list?.current !== undefined &&
      shownVersion !== list.current.version,
  );
  const extension = $derived(/\.[^./\\]+$/.exec(file.name)?.[0] ?? '');

  const address = frameAddress(document);
  const compact = $derived(page.stage === 'compact');
  const status = $derived(session.status);
  const framed = $derived(status.name === 'opening' || status.name === 'shown');
  /**
   * What "Edit" opens the editor on: the Hangul document on screen, for who may write its
   * versions, once it is shown and its version is known (a new version is written from it).
   * Not an earlier version: an edit of one could not become a version.
   */
  const editable = $derived.by(() => {
    if (!onEdit || !versions?.canWrite || status.name !== 'shown' || older) return undefined;
    const { format } = status;
    if ((format !== 'hwp' && format !== 'hwpx') || shownVersion === undefined) return undefined;
    return session.blob ? { format, blob: session.blob, version: shownVersion } : undefined;
  });
  /** The sheet shown, when the file is a workbook. */
  const sheet = $derived(session.sheets[session.page - 1]);
  /** A deck's pages are its slides, and are called so. */
  const slides = $derived(status.name === 'shown' && status.format === 'pptx');
  /** A deck's slides, when some are hidden from a show. */
  const order = $derived(slides ? session.slides : undefined);
  const onHidden = $derived(order?.hidden.has(session.page) === true);
  const where = $derived(
    sheet
      ? m.viewer_sheet({ page: session.page, pages: session.pages, name: sheet.name })
      : (onHidden ? m.viewer_slide_hidden : slides ? m.viewer_slide : m.viewer_page)({
          page: session.page,
          pages: session.pages,
        }),
  );
  /** What is said of the page turned to: of a sheet with drawings, that they are not shown. */
  const announced = $derived(sheet?.drawings ? `${where} ${m.viewer_sheet_drawings()}` : where);
  const previous = $derived(
    sheet
      ? m.viewer_previous_sheet()
      : slides
        ? m.viewer_previous_slide()
        : m.viewer_previous_page(),
  );
  const next = $derived(
    sheet ? m.viewer_next_sheet() : slides ? m.viewer_next_slide() : m.viewer_next_page(),
  );

  closeOnBack(
    () => true,
    () => {
      onClose();
    },
  );
  // Narrow, the list is over the document: back shows the document again.
  closeOnBack(
    () => compact && listed,
    () => {
      closeVersions();
    },
  );

  $effect(() => {
    const theme = display.resolved.theme;
    // Only the theme sends it again; the session's status is read, not followed.
    untrack(() => {
      session.setTheme(theme);
    });
  });

  // A session ends when another takes its place, and with the viewer.
  $effect(() => {
    const current = session;
    return () => {
      current.close();
    };
  });

  function download(): void {
    if (session.blob) {
      saveBlob(session.blob, file.name);
      return;
    }
    void session.file.read().then((blob) => {
      if (blob) saveBlob(blob, file.name);
    });
  }

  /**
   * Takes the file's lease and opens the editor on the version shown, unless
   * the file has a newer one by then: that one is shown instead.
   */
  async function edit(): Promise<void> {
    const edited = editable;
    if (!edited || !versions || !list || taking || watch?.editing) return;
    taking = true;
    editNote = undefined;
    const started = await startEditing(versions, edited.version, async () => {
      await list.read();
      return list.current?.version;
    });
    taking = false;
    if (started.as !== 'held') {
      if (closed) return;
      if (started.as === 'newer' && list.current) view(list.current);
      if (started.as !== 'editing') editNote = started.as;
      void watch?.read();
      return;
    }
    const { format, blob, version } = edited;
    const bytes = await blob.arrayBuffer().catch(() => undefined);
    if (bytes && !closed) onEdit?.({ name: file.name, format, bytes, version }, started.lease);
    else void started.lease.release();
  }

  /** Shows a version in the file's place. */
  function view(version: FileVersion): void {
    const source = versions;
    // A write that ended after the viewer was closed has nowhere to be shown.
    if (!source || closed) return;
    shownVersion = version.version;
    session = sessionOf({
      name: file.name,
      size: version.size,
      read: (signal) => source.bytes(version.version, signal),
    });
    // Narrow, the document is under the list.
    if (compact) closeVersions();
  }

  function downloadVersion(version: FileVersion): void {
    void versions?.bytes(version.version).then((blob) => {
      if (blob) saveBlob(blob, file.name);
    });
  }

  async function openVersions(): Promise<void> {
    if (!list) return;
    // What it said when it was last shown may be so no more.
    void list.read();
    listed = true;
    await tick();
    panel?.querySelector<HTMLElement>('h2')?.focus();
  }

  function closeVersions(): void {
    listed = false;
    // The list is gone with its focus: back to the button it was opened from.
    document.getElementById(`${uid}-versions`)?.focus();
  }

  function command(key: string): boolean {
    switch (key) {
      case 'ArrowLeft':
        session.turn(-1);
        return true;
      case 'ArrowRight':
        session.turn(1);
        return true;
      case '+':
      case '=':
        session.zoomBy(1);
        return true;
      case '-':
        session.zoomBy(-1);
        return true;
      case '0':
        session.zoomBy(0);
        return true;
      case 'Escape':
        onClose();
        return true;
      default:
        return false;
    }
  }

  function onkeydown(event: KeyboardEvent): void {
    // Escape is the dialog's own; keys typed into a control are the control's.
    if (event.key === 'Escape' || event.ctrlKey || event.metaKey || event.altKey) return;
    // With Shift, the arrows across are the frame's, which scrolls across with them where it has
    // the focus: they turn no page from the viewer's buttons either.
    if (event.shiftKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) return;
    if (command(event.key)) event.preventDefault();
  }
</script>

<Dialog.Root
  open
  onOpenChange={(open) => {
    if (!open) onClose();
  }}
>
  <Dialog.Content
    showCloseButton={false}
    data-testid="file-viewer"
    data-status={status.name}
    data-frame={compact ? 'full' : 'dialog'}
    class={[
      'flex flex-col gap-0 overflow-hidden bg-background p-0',
      compact
        ? 'top-0 left-0 h-dvh w-full max-w-none translate-x-0 translate-y-0 rounded-none pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none'
        : 'h-[min(calc(100dvh-4rem),64rem)] max-w-[min(64rem,calc(100%-2rem))] sm:max-w-[min(64rem,calc(100%-2rem))]',
    ]}
    {onkeydown}
    onCloseAutoFocus={(event: Event) => {
      if (!returnFocus) return;
      event.preventDefault();
      returnFocus();
    }}
  >
    <header class="flex min-h-12 items-center gap-1 border-b py-1 pr-1 pl-4">
      <Dialog.Title class="min-w-0 flex-1 truncate text-base font-semibold"
        >{file.name}</Dialog.Title
      >
      <Dialog.Description class="sr-only">{m.viewer_description()}</Dialog.Description>
      {#if older}
        <p
          class="shrink-0 rounded-sm border px-1.5 py-0.5 text-xs font-medium"
          data-testid="file-viewer-older"
        >
          {m.viewer_older_version()}
        </p>
      {/if}
      {#if editable}
        <IconButton
          label={m.viewer_edit()}
          aria-disabled={taking || watch?.editing ? 'true' : undefined}
          aria-describedby={watch?.editing ? `${uid}-editing` : undefined}
          class="aria-disabled:opacity-50"
          data-testid="file-viewer-edit"
          onclick={edit}
        >
          <PencilIcon aria-hidden="true" />
        </IconButton>
      {/if}
      {#if versions}
        <IconButton
          id="{uid}-versions"
          label={m.viewer_versions()}
          aria-expanded={listed}
          data-testid="file-viewer-versions"
          onclick={() => {
            if (listed) closeVersions();
            else void openVersions();
          }}
        >
          <HistoryIcon aria-hidden="true" />
        </IconButton>
      {/if}
      <IconButton label={m.viewer_download()} data-testid="file-viewer-download" onclick={download}>
        <DownloadIcon aria-hidden="true" />
      </IconButton>
      <IconButton label={m.ui_close()} data-testid="file-viewer-close" onclick={onClose}>
        <XIcon aria-hidden="true" />
      </IconButton>
    </header>
    {#if status.name === 'shown' && order}
      <SlideCount {session} {order} />
    {/if}
    {#if watch}
      <EditingNote
        id="{uid}-editing"
        {watch}
        note={editNote}
        userName={(userId: string) => versions?.userName(userId)}
      />
    {/if}

    <div class="relative flex min-h-0 flex-1">
      <!-- Narrow, the list is over the document: nothing under it takes the focus. -->
      <div class="relative min-h-0 min-w-0 flex-1 bg-muted" inert={compact && listed}>
        {#if framed}
          <!-- A frame is one session's: another version is drawn in a new one. -->
          {#key session}
            <iframe
              {@attach session.attach}
              src={address}
              sandbox="allow-scripts"
              referrerpolicy="no-referrer"
              title={m.viewer_frame_title({ name: file.name })}
              class={['size-full border-0', status.name !== 'shown' && 'invisible']}
              data-testid="file-viewer-frame"
            ></iframe>
          {/key}
        {/if}
        <ViewerNotice
          {status}
          onDownload={download}
          onAnswer={(wait: boolean) => {
            if (wait) session.waitOn();
            else session.giveUp();
          }}
        />
      </div>
      {#if listed && list && versions}
        <div
          bind:this={panel}
          class={compact ? 'absolute inset-0 z-10 flex' : 'flex w-80 shrink-0 border-s'}
        >
          <FileVersions
            {list}
            source={versions}
            shown={shownVersion}
            accept={extension}
            {regional}
            onView={view}
            onDownload={downloadVersion}
            onWritten={view}
            onClose={closeVersions}
          />
        </div>
      {/if}
    </div>

    <!-- In the page from the start, so that the first page is announced as well as the next. -->
    <p class="sr-only" aria-live="polite" data-testid="file-viewer-announce">
      {status.name === 'shown' ? announced : ''}
    </p>

    {#if status.name === 'shown' && session.sheets.length > 0}
      <SheetTabs
        sheets={session.sheets}
        current={session.page}
        onPick={(picked: number) => {
          session.goTo(picked);
        }}
      />
    {/if}
    {#if status.name === 'shown'}
      <PageControls {session} {where} {previous} {next} />
    {/if}
  </Dialog.Content>
</Dialog.Root>
