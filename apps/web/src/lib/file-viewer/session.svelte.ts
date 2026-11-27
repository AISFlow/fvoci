/**
 * One file open in the viewer, from reading its bytes to its frame being
 * thrown away.
 *
 * The bytes are read by the app (the frame has no network), told apart by
 * what they are (and an archive held to what it says it unpacks to), and
 * handed to a new frame, whose document draws them.
 * The frame answers through the messages of `protocol.ts`; a message counts
 * only when it comes from this session's frame and has one of their shapes.
 * The frame has a time to get ready in; past it, or when the viewer closes,
 * the frame is taken out of the page, and with it its document, its worker and
 * the memory they held. It has another time to draw the file in; past that,
 * the reader is asked whether to wait on (the frame goes on drawing, and they
 * are asked again a time later) or to download the file instead (the frame
 * goes as it does past the first).
 *
 * A deck with slides hidden from a show opens on the others; the reader may
 * show the hidden ones in place for as long as the viewer is open (it is
 * not kept). Pages are turned past the slides left out (`SlideOrder`).
 */
import { tick } from 'svelte';

import { m } from '$lib/i18n/paraglide/messages.js';
import { focusRevealed } from '$lib/focus.ts';
import { buildUrl, normalizeBasePath } from '$lib/logic/urls.ts';

import { ENGINE_FETCH_LIMIT, engineBytes } from './engine.ts';
import { examine, needsEngine, VIEW_LIMIT_BYTES, type FormatId } from './formats.ts';
import {
  readFromFrame,
  ZOOM_MAX,
  ZOOM_MIN,
  type Failure,
  type SheetSummary,
  type Theme,
  type ToFrame,
  type ViewerKey,
} from './protocol.ts';
import { SlideOrder } from './slide-order.ts';

/** A file to view: what is known before its bytes are read, and how to read them. */
export interface ViewedFile {
  name: string;
  /** Its size as recorded, in bytes; 0 when unknown. */
  size: number;
  /**
   * Its bytes; undefined when they could not be read (the reason is told
   * elsewhere) or the reading was given up through `signal`.
   */
  read: (signal?: AbortSignal) => Promise<Blob | undefined>;
}

export interface Limits {
  /** The largest file opened, in bytes. */
  bytes: number;
  /** How long the frame may take to get ready, in milliseconds. */
  ready: number;
  /** How long it draws the file once it has it before the reader is asked whether to wait on. */
  draw: number;
  /**
   * How long the app may take to fetch rhwp's engine for a Hangul document, the first of a
   * visit (7 MB on the wire): its own time, before the frame's time to draw starts.
   */
  engine: number;
}

export const LIMITS: Limits = {
  bytes: VIEW_LIMIT_BYTES,
  ready: 15_000,
  draw: 30_000,
  engine: ENGINE_FETCH_LIMIT,
};

export type Status =
  | { name: 'reading' }
  /** The file is over the limit, or an archive that unpacks past the limits. */
  | { name: 'too-large'; what: 'file' | 'contents' }
  | { name: 'unknown' }
  /** `late`: the file was not drawn in its time, and the reader is asked whether to wait on. */
  | { name: 'opening'; format: FormatId; late: boolean }
  | { name: 'shown'; format: FormatId }
  | { name: 'failed'; reason: Failure | 'unread' | 'slow' | BeforeFile };

/**
 * Why the frame was given up before it had the file: it did not say it is
 * ready in its time (`unstarted`: its page or scripts not loaded, as where a
 * gate in front of the server answers without its cookie, or failing as they
 * run), or the engine it draws with did not come (`engine`: late, refused, or
 * not WebAssembly). Each is told apart from a file too slow to draw (`slow`).
 */
export type BeforeFile = 'unstarted' | 'engine';

/** The steps of the zoom buttons. */
const ZOOMS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4].filter(
  (zoom) => zoom >= ZOOM_MIN && zoom <= ZOOM_MAX,
);

/** The address of the frame's document, under the path the app is served at. */
export function frameAddress(document: Document): string {
  const raw = document.querySelector('meta[name="fvoci:base"]')?.getAttribute('content') ?? '';
  return buildUrl('/static/viewer-frame.html', raw.startsWith('{{') ? '' : normalizeBasePath(raw));
}

export class ViewerSession {
  status = $state.raw<Status>({ name: 'reading' });
  pages = $state(0);
  page = $state(1);
  /** A workbook's sheets, by name: its pages. None for a document of pages. */
  sheets = $state.raw<readonly SheetSummary[]>([]);
  /** A deck's slides when some are hidden from a show; undefined for any other document. */
  slides = $state.raw<SlideOrder | undefined>(undefined);
  /** Whether a deck's hidden slides are shown. */
  showHidden = $state(false);
  zoom = $state(1);
  /** The file's bytes once read: what "Download" saves. */
  blob = $state.raw<Blob | undefined>(undefined);

  readonly file: ViewedFile;
  readonly #limits: Limits;
  readonly #theme: () => Theme;
  readonly #lang: () => string;
  readonly #onKey: (key: ViewerKey) => void;
  readonly #onDownload: () => void;
  readonly #engine: () => Promise<ArrayBuffer>;
  /** Gives up the reading, and stands for "closed". */
  readonly #abort = new AbortController();
  #bytes: ArrayBuffer | undefined;
  /** The hidden slides the frame told of, until it tells how many pages there are. */
  #hidden: readonly number[] = [];
  #frame: HTMLIFrameElement | undefined;
  #timer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    file: ViewedFile,
    options: {
      theme: () => Theme;
      /** The app's language, for the document's text. */
      lang: () => string;
      onKey: (key: ViewerKey) => void;
      /** The reader asked, in the frame, for the file to download. */
      onDownload: () => void;
      limits?: Limits;
      /** rhwp's engine, a copy to hand over; the app's own fetch of it unless given. */
      engine?: () => Promise<ArrayBuffer>;
    },
  ) {
    this.file = file;
    this.#theme = options.theme;
    this.#lang = options.lang;
    this.#onKey = options.onKey;
    this.#onDownload = options.onDownload;
    this.#limits = options.limits ?? LIMITS;
    this.#engine = options.engine ?? engineBytes;
    this.#read().catch(() => {
      // A reader of the bytes that threw rather than answering undefined.
      if (!this.#gone()) this.#fail('unread');
    });
  }

  async #read(): Promise<void> {
    if (this.file.size > this.#limits.bytes) {
      this.status = { name: 'too-large', what: 'file' };
      return;
    }
    const blob = await this.file.read(this.#abort.signal);
    if (this.#gone()) return;
    if (!blob) {
      this.status = { name: 'failed', reason: 'unread' };
      return;
    }
    this.blob = blob;
    if (blob.size > this.#limits.bytes) {
      this.status = { name: 'too-large', what: 'file' };
      return;
    }
    const bytes = await blob.arrayBuffer();
    if (this.#gone()) return;
    const examined = examine(new Uint8Array(bytes));
    if (examined.kind === 'unknown') {
      this.status = { name: 'unknown' };
      return;
    }
    if (examined.kind === 'too-large') {
      this.status = { name: 'too-large', what: 'contents' };
      return;
    }
    this.#bytes = bytes;
    this.status = { name: 'opening', format: examined.format, late: false };
  }

  /** For `{@attach}` on the frame: listens to it while it is in the page. */
  attach = (frame: HTMLIFrameElement): (() => void) => {
    this.#frame = frame;
    const onMessage = (event: MessageEvent): void => {
      if (event.source === null || event.source !== frame.contentWindow) return;
      this.#receive(event.data);
    };
    // The frame's document loads once. Another load is the frame gone
    // somewhere else (the frame stops a document's links; this is the second
    // guard): whatever is in it now is not ours to show.
    let loads = 0;
    const onLoad = (): void => {
      loads += 1;
      if (loads > 1) {
        frame.src = 'about:blank';
        frame.remove();
        this.#fail('renderer');
      }
    };
    window.addEventListener('message', onMessage);
    frame.addEventListener('load', onLoad);
    this.#wait(this.#limits.ready, 'unstarted');
    return () => {
      window.removeEventListener('message', onMessage);
      frame.removeEventListener('load', onLoad);
      clearTimeout(this.#timer);
      if (this.#frame === frame) this.#frame = undefined;
    };
  };

  /** `then`: what the frame not done in `milliseconds` comes to, given up or the reader asked. */
  #wait(milliseconds: number, then: BeforeFile | 'late'): void {
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => {
      const status = this.status;
      if (then !== 'late') this.#fail(then);
      else if (status.name === 'opening') this.status = { ...status, late: true };
    }, milliseconds);
  }

  /** The reader waits on for a file not drawn in its time: the frame goes on, for another such time. */
  waitOn(): void {
    const status = this.status;
    if (status.name !== 'opening' || !status.late) return;
    this.status = { ...status, late: false };
    this.#wait(this.#limits.draw, 'late');
  }

  /** The reader takes the file rather than wait on for it: the frame goes, as one too slow. */
  giveUp(): void {
    const status = this.status;
    if (status.name === 'opening' && status.late) this.#fail('slow');
  }

  /** Taking the status off "opening" and "shown" takes the frame out of the page. */
  #fail(reason: Extract<Status, { name: 'failed' }>['reason']): void {
    clearTimeout(this.#timer);
    this.status = { name: 'failed', reason };
  }

  #send(message: ToFrame, transfer: Transferable[] = []): void {
    // The frame's origin is opaque, so there is no origin to name; what goes
    // to it is the file the reader asked to see.
    this.#frame?.contentWindow?.postMessage(message, '*', transfer);
  }

  #receive(data: unknown): void {
    const message = readFromFrame(data);
    const status = this.status;
    if (!message || (status.name !== 'opening' && status.name !== 'shown')) return;
    switch (message.kind) {
      case 'ready': {
        const bytes = this.#bytes;
        if (status.name !== 'opening' || !bytes) return;
        // Handed over, not copied: the app keeps the blob for "Download".
        this.#bytes = undefined;
        void this.#open(status.format, bytes);
        break;
      }
      case 'sheets':
        if (status.name === 'opening') this.sheets = message.sheets;
        break;
      case 'hidden':
        // Of a deck only: another document's pages are turned one by one.
        if (status.name === 'opening' && status.format === 'pptx') this.#hidden = message.pages;
        break;
      case 'shown':
        clearTimeout(this.#timer);
        // Sheets that are not the pages shown are not shown as them.
        if (this.sheets.length !== message.pages) this.sheets = [];
        this.pages = message.pages;
        // Hidden slides past the last page are not of these pages: none is taken as hidden.
        this.slides =
          (this.#hidden.at(-1) ?? Infinity) <= message.pages
            ? new SlideOrder(message.pages, this.#hidden)
            : undefined;
        this.page = this.slides?.first(false) ?? 1;
        this.status = { name: 'shown', format: status.format };
        // The keys that scroll a page (arrows, Page Down, Space) then scroll the document.
        // After the update: until then the frame is hidden, and takes no focus.
        void tick().then(() => {
          focusRevealed(this.#frame);
        });
        break;
      case 'page':
        // A slide left out is not the reader's (a message sent before they were left out).
        if (this.slides && !this.slides.shows(message.page, this.showHidden)) break;
        this.page = Math.min(message.page, this.pages);
        break;
      case 'failed':
        this.#fail(message.reason);
        break;
      case 'key':
        this.#onKey(message.key);
        break;
      case 'download':
        // From a button in a page the frame shows, so only once it shows the document.
        if (status.name === 'shown') this.#onDownload();
        break;
    }
  }

  /**
   * Hands the frame the file, and with a Hangul document rhwp's engine, which the app fetches
   * first; the frame's time to draw starts when it has both.
   */
  async #open(format: FormatId, bytes: ArrayBuffer): Promise<void> {
    let engine: ArrayBuffer | null = null;
    if (needsEngine(format)) {
      this.#wait(this.#limits.engine, 'engine');
      try {
        engine = await this.#engine();
      } catch {
        if (this.status.name === 'opening') this.#fail('engine');
        return;
      }
      // Closed, or given up on, while the engine was on its way.
      if (this.#gone() || this.status.name !== 'opening') return;
    }
    const open = { format, bytes, engine, theme: this.#theme(), lang: this.#lang() };
    const labels = {
      chart: m.viewer_chart_not_drawn(),
      pageTooLarge: m.viewer_page_too_large(),
      download: m.viewer_download(),
      hidden: m.viewer_hidden(),
    };
    this.#send({ kind: 'open', ...open, labels }, engine ? [bytes, engine] : [bytes]);
    this.#wait(this.#limits.draw, 'late');
  }

  goTo(page: number): void {
    if (this.status.name !== 'shown') return;
    const target = Math.min(Math.max(1, page), this.pages);
    this.page = target;
    this.#send({ kind: 'page', page: target });
  }

  /** The page after (1) or before (-1) the one in view, past the slides left out; undefined at an end. */
  #turned(step: 1 | -1): number | undefined {
    if (this.slides) return this.slides.next(this.page, step, this.showHidden);
    const page = this.page + step;
    return page >= 1 && page <= this.pages ? page : undefined;
  }

  canTurn(step: 1 | -1): boolean {
    return this.#turned(step) !== undefined;
  }

  /** Turns to the next page (1) or the previous one (-1). */
  turn(step: 1 | -1): void {
    const page = this.#turned(step);
    if (page !== undefined) this.goTo(page);
  }

  /** Shows a deck's hidden slides in place, or leaves them out again. */
  toggleHidden(): void {
    const slides = this.slides;
    if (this.status.name !== 'shown' || !slides || slides.allHidden) return;
    this.showHidden = !this.showHidden;
    // Where the reader lands, until the frame tells the slide it keeps in view.
    this.page = slides.landing(this.page, this.showHidden);
    this.#send({ kind: 'hidden', shown: this.showHidden });
  }

  /** Zooms one step in (1) or out (-1), or back to 1 (0). */
  zoomBy(step: -1 | 0 | 1): void {
    if (this.status.name !== 'shown') return;
    const at = ZOOMS.indexOf(this.zoom);
    const next = step === 0 ? 1 : (ZOOMS[Math.min(ZOOMS.length - 1, Math.max(0, at + step))] ?? 1);
    this.zoom = next;
    this.#send({ kind: 'zoom', zoom: next });
  }

  get canZoomIn(): boolean {
    return this.zoom < (ZOOMS.at(-1) ?? 1);
  }

  get canZoomOut(): boolean {
    return this.zoom > (ZOOMS[0] ?? 1);
  }

  setTheme(theme: Theme): void {
    if (this.status.name === 'shown' || this.status.name === 'opening') {
      this.#send({ kind: 'theme', theme });
    }
  }

  /** Whether the viewer was closed meanwhile. */
  #gone(): boolean {
    return this.#abort.signal.aborted;
  }

  /** Stops everything: the reading, the timers; the frame goes with the component that holds it. */
  close(): void {
    this.#abort.abort();
    this.#bytes = undefined;
    clearTimeout(this.#timer);
  }
}
