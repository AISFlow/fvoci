/**
 * One version of an HWP or HWPX file open in the editor, from handing its
 * bytes to the editor's frame to the frame being thrown away, and the saving
 * of what was edited.
 *
 * The frame (src/editor-frame/) edits; it stores nothing and reaches nobody.
 * Saving is done here, through the file's versions
 * (versions.svelte.ts):
 *
 * 1. The frame is asked for the document. It answers with the bytes and
 *    rhwp's report of what those bytes lose of the document, in one message;
 *    a report that cannot be read is no answer, and nothing is saved.
 * 2. The bytes must be a file of the format that was opened, and no larger
 *    than the viewer opens: what the frame hands back is not trusted to be a
 *    document because the frame says so.
 * 3. When the report names losses, or the document is a distribution
 *    document (배포용 문서) that the saved file is not, nothing is saved until
 *    the writer has seen what and chosen: a new version anyway, a file of its
 *    own beside this one (which keeps every version of this one as it is), or
 *    neither.
 * 4. The bytes are uploaded and made the file's new version, written from the
 *    version that was opened. When the file has a newer one by then, nothing
 *    of the file is changed: the writer is asked whether to keep the edit as
 *    a file of its own, and what was uploaded is used for it.
 *
 * A save under way is finished even when the editor is closed meanwhile; if
 * the file got a newer version by then, the edit is kept as a file of its own
 * rather than dropped, as there is nobody left to ask.
 */
import { tick } from 'svelte';

import { focusRevealed } from '$lib/focus.ts';

import {
  readFromEditor,
  REQUEST_LIMIT,
  type EditFailure,
  type EditFormat,
  type FromEditor,
  type Loss,
  type Recount,
  type ToEditor,
} from './editor-protocol.ts';
import { ENGINE_FETCH_LIMIT, engineBytes } from './engine.ts';
import { examine, VIEW_LIMIT_BYTES } from './formats.ts';
import type { Theme } from './protocol.ts';
import { frameAddress, type BeforeFile } from './session.svelte.ts';
import type { FileVersion, VersionsSource, VersionUpload } from './versions.svelte.ts';

/** The document handed to the editor. */
export interface EditedFile {
  name: string;
  format: EditFormat;
  /** The bytes of the version being edited. */
  bytes: ArrayBuffer;
  /** That version's number: what a new version is written from. */
  version: number;
}

export interface EditLimits {
  /** The largest file saved, in bytes. */
  bytes: number;
  /** How long the frame may take to get ready, to open the document, and to export it, in milliseconds. */
  ready: number;
  open: number;
  export: number;
  /** How long the app may take to fetch rhwp's engine, when it has not yet. */
  engine: number;
}

export const EDIT_LIMITS: EditLimits = {
  bytes: VIEW_LIMIT_BYTES,
  ready: 15_000,
  open: 60_000,
  export: 60_000,
  engine: ENGINE_FETCH_LIMIT,
};

export type EditStatus =
  | { name: 'opening' }
  | { name: 'ready'; pages: number }
  | { name: 'failed'; reason: EditFailure | 'slow' | BeforeFile };

/** Why a save did not go through. */
export type SaveFailure =
  /** The editor gave no document, or no report of what it loses. */
  | 'export'
  /** What it gave is not a file of the format opened, or is larger than is saved. */
  | 'not-a-document'
  | 'too-large'
  /** It did not go up, or the server refused it: the stack of uploads says why. */
  | 'upload'
  /** The reader gave its upload up, in the stack of uploads. */
  | 'stopped'
  | 'refused';

/** What was exported and waits for the writer's choice. */
interface Exported {
  request: number;
  file: File;
}

export type Saving =
  | { name: 'idle' }
  | { name: 'exporting' }
  /** A save asked from inside the sandboxed frame always confirms in the parent UI first. */
  | { name: 'confirm'; exported: Exported }
  /**
   * The saved file loses something (as rhwp says, or the frame finds), takes a distribution
   * document's protection away, or reads back with other counts of pages or controls than the
   * document in the editor: the writer chooses.
   */
  | {
      name: 'lossy';
      count: number;
      losses: readonly Loss[];
      unprotected: boolean;
      recounts: readonly Recount[];
      exported: Exported;
    }
  | { name: 'storing' }
  /** The file got a newer version meanwhile: the writer chooses. */
  | { name: 'stale'; request: number; upload: VersionUpload }
  | { name: 'failed'; reason: SaveFailure };

/** What the last save came to. */
export type Saved =
  | { as: 'version'; version: FileVersion }
  /** A file of its own beside the edited one, by this name. */
  | { as: 'apart'; name: string };

const MIME: Record<EditFormat, string> = {
  hwp: 'application/x-hwp',
  hwpx: 'application/hwp+zip',
};

/** The longest name the server takes for a file, in characters. */
export const NAME_LIMIT = 255;

/**
 * The name a file of its own is kept under: `name` with its stem marked as an edit by
 * `mark`, its extension kept, and the stem cut short where the whole would be longer
 * than the server takes.
 */
export function markedName(name: string, mark: (stem: string) => string): string {
  const dot = name.lastIndexOf('.');
  const [stem, extension] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ''];
  const over = Array.from(mark(stem) + extension).length - NAME_LIMIT;
  const kept = over > 0 ? Array.from(stem).slice(0, -over).join('') : stem;
  return mark(kept) + extension;
}

/** The address of the editor's frame: the editor's profile for a host that owns opening and saving. */
export function editorAddress(document: Document): string {
  return `${frameAddress(document).replace('viewer-frame.html', 'editor-frame.html')}?chrome=embed`;
}

export class EditingSession {
  status = $state.raw<EditStatus>({ name: 'opening' });
  /** The document has changes that are not stored. */
  dirty = $state(false);
  saving = $state.raw<Saving>({ name: 'idle' });
  saved = $state.raw<Saved>();

  readonly file: EditedFile;
  readonly #source: VersionsSource;
  readonly #limits: EditLimits;
  readonly #theme: () => Theme;
  readonly #lang: () => string;
  /** The name a file of its own is kept under, from the edited file's. */
  readonly #apartName: (name: string) => string;
  readonly #engine: () => Promise<ArrayBuffer>;
  readonly #onLeave: () => void;
  readonly #onInput: () => void;
  /** The version a new one is written from: the one opened, then each one written here. */
  #base: number;
  #bytes: ArrayBuffer | undefined;
  #frame: HTMLIFrameElement | undefined;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #request = 0;
  #fromFrame = false;
  #closed = false;

  constructor(
    file: EditedFile,
    source: VersionsSource,
    options: {
      theme: () => Theme;
      lang: () => string;
      apartName: (name: string) => string;
      limits?: EditLimits;
      /** rhwp's engine, a copy to hand over; the app's own fetch of it unless given. */
      engine?: () => Promise<ArrayBuffer>;
      /** The writer asked, in the document, for the app's controls (F6). */
      onLeave?: () => void;
      /** The writer is at work in the document: a key, the pointer, a change. */
      onInput?: () => void;
    },
  ) {
    this.file = file;
    this.#bytes = file.bytes;
    this.#base = file.version;
    this.#source = source;
    this.#theme = options.theme;
    this.#lang = options.lang;
    this.#apartName = options.apartName;
    this.#limits = options.limits ?? EDIT_LIMITS;
    this.#engine = options.engine ?? engineBytes;
    this.#onLeave = options.onLeave ?? (() => undefined);
    this.#onInput = options.onInput ?? (() => undefined);
  }

  /** A save is under way or waits for a choice: the editor is not closed over it. */
  get busy(): boolean {
    return this.saving.name !== 'idle' && this.saving.name !== 'failed';
  }

  /** For `{@attach}` on the frame: listens to it while it is in the page. */
  attach = (frame: HTMLIFrameElement): (() => void) => {
    this.#frame = frame;
    const onMessage = (event: MessageEvent): void => {
      if (event.source === null || event.source !== frame.contentWindow) return;
      this.#receive(event.data);
    };
    // The frame's document loads once: another load is the frame gone somewhere else.
    let loads = 0;
    const onLoad = (): void => {
      loads += 1;
      if (loads > 1) {
        frame.src = 'about:blank';
        frame.remove();
        this.#fail('editor');
      }
    };
    window.addEventListener('message', onMessage);
    frame.addEventListener('load', onLoad);
    this.#wait(this.#limits.ready, () => {
      this.#fail('unstarted');
    });
    return () => {
      window.removeEventListener('message', onMessage);
      frame.removeEventListener('load', onLoad);
      if (this.#frame === frame) this.#frame = undefined;
    };
  };

  #wait(milliseconds: number, late: () => void): void {
    clearTimeout(this.#timer);
    this.#timer = setTimeout(late, milliseconds);
  }

  #fail(reason: Extract<EditStatus, { name: 'failed' }>['reason']): void {
    clearTimeout(this.#timer);
    this.status = { name: 'failed', reason };
    // An export that was asked of a frame that is gone is not coming.
    if (this.saving.name === 'exporting') this.saving = { name: 'failed', reason: 'export' };
  }

  #send(message: ToEditor, transfer: Transferable[] = []): void {
    // The frame's origin is opaque, so there is no origin to name; what goes
    // to it is the document the writer asked to edit.
    this.#frame?.contentWindow?.postMessage(message, '*', transfer);
  }

  #receive(data: unknown): void {
    const message = readFromEditor(data);
    if (!message || this.status.name === 'failed') return;
    switch (message.kind) {
      case 'ready': {
        const bytes = this.#bytes;
        if (this.status.name !== 'opening' || !bytes) return;
        this.#bytes = undefined;
        void this.#open(bytes);
        break;
      }
      case 'opened':
        if (this.status.name !== 'opening') return;
        clearTimeout(this.#timer);
        this.status = { name: 'ready', pages: message.pages };
        // What is typed then goes to the document. After the update: until then the frame is
        // hidden, and takes no focus.
        this.#focusFrame();
        break;
      case 'failed':
        if (this.status.name === 'opening') this.#fail(message.reason);
        break;
      case 'changed':
        if (this.status.name === 'ready') this.dirty = message.dirty;
        break;
      case 'save-asked':
        // A script inside the frame cannot silently trigger a save without active user input,
        // and frame-originated exports still require parent confirmation (`confirm`).
        if ('userActivation' in navigator && navigator.userActivation.isActive) {
          this.#startSave(true);
        }
        break;
      case 'leave':
        this.#onLeave();
        break;
      case 'input':
        this.#onInput();
        break;
      case 'exported':
        if (this.saving.name === 'exporting' && message.request === this.#request) {
          clearTimeout(this.#timer);
          this.#exported(message);
        }
        break;
      case 'export-failed':
        if (this.saving.name === 'exporting' && message.request === this.#request) {
          clearTimeout(this.#timer);
          this.saving = { name: 'failed', reason: 'export' };
        }
        break;
    }
  }

  /** Hands the frame the document and rhwp's engine; its time to open starts when it has both. */
  async #open(bytes: ArrayBuffer): Promise<void> {
    this.#wait(this.#limits.engine, () => {
      this.#fail('engine');
    });
    let engine: ArrayBuffer;
    try {
      engine = await this.#engine();
    } catch {
      if (this.status.name === 'opening') this.#fail('engine');
      return;
    }
    // Closed, or given up on, while the engine was on its way.
    if (this.#closed || this.status.name !== 'opening') return;
    const { format, name } = this.file;
    const open = { format, name, bytes, engine, theme: this.#theme(), lang: this.#lang() };
    this.#send({ kind: 'open', ...open }, [bytes, engine]);
    this.#wait(this.#limits.open, () => {
      this.#fail('slow');
    });
  }

  /** Asks the editor for the document, to save it from the parent UI. */
  save(): void {
    this.#startSave(false);
  }

  #startSave(fromFrame: boolean): void {
    if (this.status.name !== 'ready' || this.busy || this.#request >= REQUEST_LIMIT) return;
    this.#request += 1;
    this.#fromFrame = fromFrame;
    this.saved = undefined;
    this.saving = { name: 'exporting' };
    this.#send({ kind: 'export', request: this.#request });
    this.#wait(this.#limits.export, () => {
      if (this.saving.name === 'exporting') this.saving = { name: 'failed', reason: 'export' };
    });
  }

  #exported({
    request,
    bytes,
    count,
    losses,
    unprotected,
    recounts,
  }: Extract<FromEditor, { kind: 'exported' }>): void {
    if (bytes.byteLength > this.#limits.bytes) {
      this.saving = { name: 'failed', reason: 'too-large' };
      return;
    }
    const examined = examine(new Uint8Array(bytes));
    if (examined.kind !== 'format' || examined.format !== this.file.format) {
      this.saving = { name: 'failed', reason: 'not-a-document' };
      return;
    }
    const file = new File([bytes], this.file.name, { type: MIME[this.file.format] });
    const exported = { request, file };
    if (count > 0 || unprotected || recounts.length > 0) {
      this.saving = { name: 'lossy', count, losses, unprotected, recounts, exported };
      return;
    }
    if (this.#fromFrame) {
      this.saving = { name: 'confirm', exported };
      return;
    }
    void this.#store(exported, 'version');
  }

  /** Confirms a save that started inside the editor frame. */
  confirmSave(): void {
    if (this.saving.name === 'confirm') void this.#store(this.saving.exported, 'version');
  }

  /** The writer saw what the saved file loses, and saves it as a new version all the same. */
  saveWithLosses(): void {
    if (this.saving.name === 'lossy') void this.#store(this.saving.exported, 'version');
  }

  /**
   * Keeps the edit as a file of its own beside the edited one: of what loses
   * something, instead of a version; of what could not become a version
   * because the file got a newer one, what was uploaded for it.
   */
  saveApart(): void {
    const saving = this.saving;
    if (saving.name === 'lossy' || saving.name === 'confirm')
      void this.#store(saving.exported, 'apart');
    else if (saving.name === 'stale') void this.#attachApart(saving.request, saving.upload);
  }

  /** Leaves a choice unmade, or a failure read: nothing was saved, the document stays changed. */
  dismiss(): void {
    const { name } = this.saving;
    if (name === 'confirm' || name === 'lossy' || name === 'stale' || name === 'failed') {
      this.saving = { name: 'idle' };
    }
  }

  async #store(exported: Exported, as: 'version' | 'apart'): Promise<void> {
    this.saving = { name: 'storing' };
    const { request } = exported;
    // A file of its own goes up under its own name.
    const file =
      as === 'apart'
        ? new File([exported.file], this.#apartName(exported.file.name), exported.file)
        : exported.file;
    const upload = await this.#source.upload(file).catch(() => undefined);
    if (!upload || upload === 'stopped') {
      this.saving = { name: 'failed', reason: upload ?? 'upload' };
      return;
    }
    if (as === 'apart') {
      await this.#attachApart(request, upload);
      return;
    }
    try {
      const version = await this.#source.add(this.#base, upload);
      this.#base = version.version;
      this.#stored(request, { as: 'version', version });
    } catch (error) {
      if (this.#source.refusalOf(error) !== 'stale') {
        this.saving = { name: 'failed', reason: 'refused' };
      } else if (this.#closed) {
        // Nobody is left to ask: the edit is kept rather than dropped.
        await this.#attachApart(request, upload);
      } else {
        this.saving = { name: 'stale', request, upload };
      }
    }
  }

  async #attachApart(request: number, upload: VersionUpload): Promise<void> {
    this.saving = { name: 'storing' };
    // What went up for a version has the edited file's name; what went up apart has its own.
    const name = this.#apartName(this.file.name);
    try {
      await this.#source.attachApart({ ...upload, name });
      this.#stored(request, { as: 'apart', name });
    } catch {
      this.saving = { name: 'failed', reason: 'refused' };
    }
  }

  #stored(request: number, saved: Saved): void {
    this.saved = saved;
    this.saving = { name: 'idle' };
    // The frame marks the document unchanged only if nothing changed since it exported it.
    this.#send({ kind: 'stored', request, as: saved.as });
    // Back to the document from the button or the dialog the save was asked with.
    this.#focusFrame();
  }

  /** Moves the focus to the frame once Svelte has made it visible. */
  #focusFrame(): void {
    void tick().then(() => {
      focusRevealed(this.#frame);
    });
  }

  /** Stops the timers; the frame goes with the component that holds it. A save under way goes on. */
  close(): void {
    this.#closed = true;
    this.#bytes = undefined;
    clearTimeout(this.#timer);
  }
}
