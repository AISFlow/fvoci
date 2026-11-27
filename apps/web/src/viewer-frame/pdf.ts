/**
 * PDF, with pdf.js.
 *
 * The pages are laid out as boxes of their size at once, and only those in
 * view (and a screen's height around them) are drawn: a canvas for the page
 * and pdf.js's text layer over it, so that text can be selected and a screen
 * reader finds it. A page that scrolls far away gives its canvas back.
 *
 * pdf.js reads the document in a worker, so that a large file does not
 * stop the frame (and with it, in browsers that run the frame in the app's
 * thread, the app). The frame cannot start a worker from an address: its
 * origin is opaque, and in WebKit a `blob:` worker may not load scripts from
 * the app either. So the worker's whole code is built into this chunk and
 * started from a `blob:` (`pdf-worker.ts`, `?worker&inline`). The development
 * server cannot build it so; there the reading happens in the frame itself.
 */
import {
  getDocument,
  GlobalWorkerOptions,
  InvalidPDFException,
  PasswordException,
  RenderingCancelledException,
  TextLayer,
  type PDFDocumentProxy,
  type PDFPageProxy,
  type RenderTask,
} from 'pdfjs-dist';

import { PAGE_LIMIT } from '$lib/file-viewer/protocol.ts';

import { BUNDLED_DATA_OPTIONS } from './pdf-data.ts';
import PdfWorker from './pdf-worker.ts?worker&inline';
import {
  PageFollower,
  LockedFile,
  UnreadableFile,
  type Drawn,
  type Render,
  type Report,
} from './renderer.ts';

/** CSS pixels per PDF unit (1/72 inch at 96 CSS pixels per inch). */
const CSS_PER_PDF_UNIT = 96 / 72;
/** A canvas holds at most this many pixels; Safari refuses larger ones. */
const CANVAS_PIXELS = 16 * 1024 * 1024;
/** The room around the pages, in CSS pixels, at each side. */
const GUTTER = 16;

/** Starts pdf.js's worker; the frame shows one document, so one worker serves it. */
async function startWorker(): Promise<void> {
  if (import.meta.env.DEV) {
    // pdf.js finds the worker's handler on the global object and runs it here.
    await import('pdfjs-dist/build/pdf.worker.mjs');
    return;
  }
  GlobalWorkerOptions.workerPort ??= new PdfWorker();
}

async function open(bytes: ArrayBuffer): Promise<PDFDocumentProxy> {
  await startWorker();
  // Dynamic code evaluation is blocked by the viewer frame's CSP (`script-src` omits `'unsafe-eval'`).
  const task = getDocument({
    data: new Uint8Array(bytes),
    ...BUNDLED_DATA_OPTIONS,
    enableXfa: false,
    stopAtErrors: false,
  });
  try {
    return await task.promise;
  } catch (error) {
    if (error instanceof PasswordException) throw new LockedFile();
    if (error instanceof InvalidPDFException) throw new UnreadableFile();
    throw error;
  }
}

/** One page: its box in the column, and what is drawn in it while it is in view. */
class Page {
  readonly box = document.createElement('div');
  #proxy: PDFPageProxy | undefined;
  #task: RenderTask | undefined;
  #text: TextLayer | undefined;
  /** The scale the page is drawn at, or 0 when nothing is drawn. */
  #drawnAt = 0;
  /** Counts the draws and clears: a draw that is no longer the last one stops. */
  #turn = 0;
  /** The draw at `#drawnAt` while it runs and once it is done. */
  #drawing: Promise<void> = Promise.resolve();

  readonly number: number;

  constructor(number: number) {
    this.number = number;
    this.box.className = 'page';
    this.box.dataset.page = String(number);
  }

  /**
   * Sizes the box for `scale` (CSS pixels per PDF unit times the zoom): from
   * the page's own size once it is known, from the size given until then.
   */
  size(width: number, height: number, scale: number): void {
    const own = this.#proxy?.getViewport({ scale: 1 });
    this.box.style.width = `${Math.floor((own?.width ?? width) * scale)}px`;
    this.box.style.height = `${Math.floor((own?.height ?? height) * scale)}px`;
  }

  /** Draws the page at `scale`; a draw at that scale already under way or done is the one waited for. */
  draw(document: PDFDocumentProxy, scale: number): Promise<void> {
    if (this.#drawnAt === scale) return this.#drawing;
    this.clear();
    this.#drawnAt = scale;
    this.#drawing = this.#paint(document, scale);
    return this.#drawing;
  }

  async #paint(document: PDFDocumentProxy, scale: number): Promise<void> {
    const turn = this.#turn;
    this.#proxy ??= await document.getPage(this.number);
    if (turn !== this.#turn) return;
    const viewport = this.#proxy.getViewport({ scale });
    const { width, height } = viewport;
    this.box.style.width = `${Math.floor(width)}px`;
    this.box.style.height = `${Math.floor(height)}px`;
    const output = Math.min(
      window.devicePixelRatio || 1,
      Math.sqrt(CANVAS_PIXELS / (width * height)),
    );
    const canvas = window.document.createElement('canvas');
    canvas.width = Math.floor(width * output);
    canvas.height = Math.floor(height * output);
    this.box.replaceChildren(canvas);
    this.#task = this.#proxy.render({
      canvas,
      viewport,
      transform: output === 1 ? undefined : [output, 0, 0, output, 0, 0],
    });
    try {
      await this.#task.promise;
    } catch (error) {
      if (error instanceof RenderingCancelledException) return;
      throw error;
    }
    if (turn !== this.#turn) return;
    const layer = window.document.createElement('div');
    layer.className = 'textLayer';
    layer.style.setProperty('--total-scale-factor', String(scale));
    this.box.append(layer);
    this.#text = new TextLayer({
      textContentSource: this.#proxy.streamTextContent(),
      container: layer,
      viewport,
    });
    await this.#text.render().catch(() => undefined);
  }

  clear(): void {
    this.#turn += 1;
    this.#task?.cancel();
    this.#text?.cancel();
    // What the worker keeps for the page (its operator list, its fonts' use) goes too.
    this.#proxy?.cleanup();
    this.#task = undefined;
    this.#text = undefined;
    this.#drawnAt = 0;
    for (const canvas of this.box.querySelectorAll('canvas')) {
      // Gives the canvas's memory back at once rather than when it is collected.
      canvas.width = 0;
      canvas.height = 0;
    }
    this.box.replaceChildren();
  }
}

class PdfView implements Drawn {
  readonly #document: PDFDocumentProxy;
  readonly #pages: Page[];
  readonly #visible = new Set<Page>();
  readonly #observer: IntersectionObserver;
  readonly #width: number;
  readonly #height: number;
  #zoom = 1;

  readonly #follower: PageFollower;

  constructor(document: PDFDocumentProxy, place: HTMLElement, first: PDFPageProxy, report: Report) {
    this.#document = document;
    const { width, height } = first.getViewport({ scale: 1 });
    this.#width = width;
    this.#height = height;
    const column = window.document.createElement('div');
    column.className = 'pages';
    this.#pages = Array.from({ length: document.numPages }, (_, index) => new Page(index + 1));
    column.append(...this.#pages.map((page) => page.box));
    place.replaceChildren(column);
    this.#observer = new IntersectionObserver(
      (entries) => {
        this.#onSight(entries);
      },
      {
        rootMargin: '100% 0px',
      },
    );
    this.#layOut();
    for (const page of this.#pages) this.#observer.observe(page.box);
    this.#follower = new PageFollower(() => this.boxes, report);
    let resizing: ReturnType<typeof setTimeout> | undefined;
    window.addEventListener('resize', () => {
      clearTimeout(resizing);
      resizing = setTimeout(() => {
        this.#layOut();
      }, 150);
    });
  }

  get boxes(): Element[] {
    return this.#pages.map((page) => page.box);
  }

  /** The scale at zoom 1: the page as wide as the frame allows, at most its printed size. */
  #scale(): number {
    const room = Math.max(1, window.innerWidth - 2 * GUTTER);
    return Math.min(CSS_PER_PDF_UNIT, room / this.#width) * this.#zoom;
  }

  #layOut(): void {
    const scale = this.#scale();
    for (const page of this.#pages) page.size(this.#width, this.#height, scale);
    for (const page of this.#visible) this.#draw(page, scale);
  }

  #draw(page: Page, scale: number): void {
    page.draw(this.#document, scale).catch(() => {
      page.clear();
    });
  }

  #onSight(entries: IntersectionObserverEntry[]): void {
    const scale = this.#scale();
    for (const entry of entries) {
      const page = this.#pages[Number((entry.target as HTMLElement).dataset.page) - 1];
      if (!page) continue;
      if (entry.isIntersecting) {
        this.#visible.add(page);
        this.#draw(page, scale);
      } else {
        this.#visible.delete(page);
        page.clear();
      }
    }
  }

  /** Draws the first page, the one in view when the document opens; rejects when it cannot be drawn. */
  drawFirst(): Promise<void> {
    return this.#pages[0]?.draw(this.#document, this.#scale()) ?? Promise.resolve();
  }

  goTo(page: number): void {
    const box = this.#pages[page - 1]?.box;
    if (!box) return;
    box.scrollIntoView({ block: 'start' });
    this.#follower.pin(page);
  }

  zoom(zoom: number): void {
    const before = document.scrollingElement;
    const ratio = before ? before.scrollTop / Math.max(1, before.scrollHeight) : 0;
    this.#zoom = zoom;
    this.#layOut();
    if (before) before.scrollTop = ratio * before.scrollHeight;
  }
}

export const render: Render = async (bytes, place, report: Report) => {
  const document = await open(bytes);
  // More pages than the viewer takes a document to have: a box for each would be laid out first.
  if (document.numPages > PAGE_LIMIT) throw new UnreadableFile();
  const first = await document.getPage(1);
  const view = new PdfView(document, place, first, report);
  // Shown once the reader sees a page, so that the time to draw counts and a page that
  // cannot be drawn is a failure rather than a white box.
  await view.drawFirst();
  report.shown(document.numPages);
  return view;
};
