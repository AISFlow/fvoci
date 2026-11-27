/**
 * The file open in the viewer: opened from a card's attachments, or from a
 * body's file block, in the editor or where a body is only shown, on any
 * screen of the app or of a public link. One at a time:
 * `FileViewerHost.svelte`, which the app's frame mounts once, shows it.
 *
 * When the viewer closes, the focus goes where the opener says (`back`):
 * the name or the link the file was opened from, or what was drawn in its
 * place meanwhile; from the editor's selected block, the editor, whose
 * selection is still the block, so that Delete deletes it and the arrows
 * move on from it. It goes there once the viewer is gone (`returnFocus`):
 * until then the viewer's dialog holds the focus in itself.
 *
 * A Hangul document's version can be handed on to the editor (`edit`), which
 * then takes the viewer's place; when the editor is closed the viewer is
 * shown again, made anew, so that it reads the file as it is by then. The
 * editor comes with the file's lease (lease.svelte.ts), which goes when it is
 * closed; the viewer asks who edits the file once the lease went (`released`).
 */
import { setFileSource, type FileSource, type FileViewerLink } from '$lib/editor/files.ts';
import type { EditedFile } from '$lib/file-viewer/editing.svelte.ts';
import type { EditLease } from '$lib/file-viewer/lease.svelte.ts';
import { opensInViewer } from '$lib/file-viewer/extensions.ts';
import type { ViewedFile } from '$lib/file-viewer/session.svelte.ts';
import type { VersionsSource } from '$lib/file-viewer/versions.svelte.ts';

import { editorChunk, viewerChunk } from './chunk.svelte.ts';

class Viewing {
  /** The file open, if any. */
  file = $state.raw<ViewedFile | undefined>(undefined);
  /** The versions of the file open, where it was opened with them (a card's attachment). */
  versions = $state.raw<VersionsSource | undefined>(undefined);
  /** The version of the file open that is in the editor, if one is. */
  edited = $state.raw<EditedFile | undefined>(undefined);
  /** The file's lease, which the editor holds while it is open. */
  lease = $state.raw<EditLease | undefined>(undefined);
  /** The last lease let go of, settled when the server answered or did not. */
  released: Promise<void> = Promise.resolve();
  /** Finds where the focus goes back to. */
  #back: (() => HTMLElement | null) | undefined;
  /** The same, for the viewer closed and not yet gone. */
  #returning: (() => HTMLElement | null) | undefined;

  /**
   * Opens the viewer on a file once it has arrived; says whether it came
   * (the connection may have dropped). A second press while one is open,
   * or on its way, opens no other.
   */
  async open(
    file: ViewedFile,
    back: () => HTMLElement | null,
    versions?: VersionsSource,
  ): Promise<boolean> {
    const arrived = await viewerChunk.load();
    if (arrived && !this.file) {
      this.#back = back;
      this.versions = versions;
      this.file = file;
    }
    return arrived;
  }

  /**
   * Opens the editor on a version of the file open, in the viewer's place,
   * once the editor has arrived, with the file's `lease`; says whether it
   * came. The lease is let go of when the editor does not open.
   */
  async edit(edited: EditedFile, lease: EditLease): Promise<boolean> {
    const file = this.file;
    const arrived = await editorChunk.load();
    // Not over a viewer closed, or opened on another file, meanwhile.
    if (arrived && file && this.file === file && this.versions && !this.edited) {
      this.lease = lease;
      this.edited = edited;
    } else {
      this.released = lease.release();
    }
    return arrived;
  }

  /** Closes the editor, and lets its lease go: the viewer shows the file again. */
  endEdit(): void {
    this.#letLeaseGo();
    this.edited = undefined;
  }

  #letLeaseGo(): void {
    if (this.lease) this.released = this.lease.release();
    this.lease = undefined;
  }

  /** Closes the viewer; the focus goes back once it is gone (`returnFocus`). */
  close(): void {
    const back = this.#back;
    this.forget();
    this.#returning = back;
  }

  /** Gives the focus back where the opener says, as the viewer closed goes. */
  returnFocus(): void {
    const back = this.#returning?.();
    this.#returning = undefined;
    if (back?.isConnected) back.focus({ preventScroll: true });
  }

  /** Lets the file go, the focus where it is: the screen it was shown over is gone. */
  forget(): void {
    this.#letLeaseGo();
    this.file = undefined;
    this.versions = undefined;
    this.edited = undefined;
    this.#back = undefined;
    this.#returning = undefined;
  }
}

export const viewing = new Viewing();

/** How the file blocks of `source`'s bodies open the viewer; none for a source that cannot read bytes. */
export function viewerLinkOf(source: Pick<FileSource, 'bytes'>): FileViewerLink | undefined {
  const bytes = source.bytes;
  if (!bytes) return undefined;
  return {
    opens: (details) => opensInViewer(details.name, details.size),
    open: (fileId, details, back) =>
      viewing.open(
        {
          name: details.name,
          size: details.size,
          // Bytes that cannot be read are the viewer's to tell; the source has reported why.
          read: (signal) => bytes(fileId, signal).catch(() => undefined),
        },
        back,
      ),
  };
}

/** Gives the bodies shown their files from `source`; their file blocks open the viewer on them. */
export function setViewedFileSource(source: FileSource): void {
  setFileSource({ ...source, view: viewerLinkOf(source) });
}
