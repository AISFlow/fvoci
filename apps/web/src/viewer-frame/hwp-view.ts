/**
 * The pages of an HWP or HWPX document in the frame (hwp.ts): laid out as
 * boxes of their size at once, and only those in view (and a screen's height
 * around them) drawn. The worker draws a page as SVG with its runs of text,
 * which hwp-page.ts copies into the box; a page that scrolls far away gives
 * its picture back. A worker stopped after the document was shown (too slow
 * over a page, or failed on its own) is reported, as no page can be drawn
 * any more. A page past what the frame takes on (hwp-document.ts) says so in
 * its place, with a button that saves the file, and the others are drawn.
 */
import type { Labels } from '$lib/file-viewer/protocol.ts';

import type { FromWorker, PageSize, TextRun, ToWorker } from './hwp-messages.ts';
import { pagePicture, pageText } from './hwp-page.ts';
import {
  LockedFile,
  PageFollower,
  SlowRenderer,
  TooLargeFile,
  UnreadableFile,
  type Drawn,
  type Report,
} from './renderer.ts';
import { WorkerFailed, type WorkerClient } from './worker-client.ts';

/**
 * Loads the app's Korean font for a page's text before the text layer is
 * measured in it (hwp-page.ts), so that each run is stretched by that font's
 * widths rather than a fallback's. A font that does not load leaves the
 * stretch to the fallback's widths.
 */
async function loadFont(runs: readonly TextRun[]): Promise<void> {
  const sample = runs.map((run) => run.text).join('');
  if (sample === '' || typeof document.fonts === 'undefined') return;
  await document.fonts.load("16px 'Noto Sans KR'", sample).catch(() => undefined);
}

/** The room around the pages, in CSS pixels, at each side. */
const GUTTER = 16;

function failure(reason: Extract<FromWorker, { kind: 'failed' }>['reason']): Error {
  if (reason === 'locked') return new LockedFile();
  if (reason === 'too-large') return new TooLargeFile();
  return reason === 'unreadable' ? new UnreadableFile() : new Error('the HWP renderer failed');
}

/** Asks the worker; resolves with its answer, rejects with what it says went wrong. */
export async function ask(
  client: WorkerClient,
  message: ToWorker,
  transfer: Transferable[] = [],
  signal?: AbortSignal,
): Promise<FromWorker> {
  const answer = (await client.request(message, transfer, signal)) as FromWorker;
  if (answer.kind === 'failed') throw failure(answer.reason);
  return answer;
}

/** What a page shows when it is past what the frame takes on: a line that says so, and a button. */
function tooLargeNotice(labels: Labels, download: () => void): HTMLElement {
  const notice = document.createElement('div');
  notice.className = 'page-notice';
  const said = document.createElement('p');
  said.textContent = labels.pageTooLarge;
  const button = document.createElement('button');
  button.type = 'button';
  button.textContent = labels.download;
  button.addEventListener('click', download);
  notice.append(said, button);
  return notice;
}

/** One page: its box in the column, and what is drawn in it while it is in view. */
class Page {
  readonly box = document.createElement('div');
  readonly number: number;
  readonly size: PageSize;
  /** What the page shows in place of a drawing that would be too large. */
  readonly #notice: () => HTMLElement;
  /** The draw under way or done, until the page is cleared. */
  #drawing: Promise<void> | undefined;
  #giveUp: AbortController | undefined;
  /** The worker said the page is too large: it is not asked again (it would draw it again). */
  #tooLarge = false;

  constructor(number: number, size: PageSize, notice: () => HTMLElement) {
    this.number = number;
    this.size = size;
    this.#notice = notice;
    this.box.className = 'page';
    this.box.dataset.page = String(number);
  }

  draw(client: WorkerClient): Promise<void> {
    if (this.#drawing) return this.#drawing;
    if (this.#tooLarge) {
      this.box.replaceChildren(this.#notice());
      this.#drawing = Promise.resolve();
      return this.#drawing;
    }
    const giveUp = new AbortController();
    this.#giveUp = giveUp;
    // Asked each time: the page may leave the view while the answer comes or the font loads.
    const gaveUp = (): boolean => giveUp.signal.aborted;
    this.#drawing = ask(client, { kind: 'page', page: this.number }, [], giveUp.signal)
      .then(async (answer) => {
        if (gaveUp() || answer.kind !== 'page') return;
        const picture = pagePicture(answer.svg, this.number, document);
        if (!picture) throw new UnreadableFile();
        await loadFont(answer.text);
        if (gaveUp()) return;
        const text = pageText(answer.text, document);
        text.style.width = `${this.size.width}px`;
        text.style.height = `${this.size.height}px`;
        this.box.replaceChildren(picture, text);
      })
      .catch((error: unknown) => {
        // A later draw of the page, once it came back into view, is not this one's to clear.
        const current = this.#giveUp === giveUp;
        if (error instanceof TooLargeFile) {
          this.#tooLarge = true;
          if (current) this.box.replaceChildren(this.#notice());
          return;
        }
        if (current) this.clear();
        throw error;
      });
    return this.#drawing;
  }

  clear(): void {
    this.#giveUp?.abort();
    this.#giveUp = undefined;
    this.#drawing = undefined;
    this.box.replaceChildren();
  }
}

/** The pages of a document the worker opened, drawn as they come into view. */
export class HwpView implements Drawn {
  readonly #client: WorkerClient;
  readonly #pages: Page[];
  readonly #column = document.createElement('div');
  readonly #report: Report;
  readonly #follower: PageFollower;
  #zoom = 1;

  constructor(
    client: WorkerClient,
    sizes: readonly PageSize[],
    place: HTMLElement,
    report: Report,
    labels: Labels,
  ) {
    this.#client = client;
    this.#report = report;
    this.#column.className = 'pages';
    const notice = (): HTMLElement =>
      tooLargeNotice(labels, () => {
        report.download();
      });
    this.#pages = sizes.map((size, index) => new Page(index + 1, size, notice));
    this.#column.append(...this.#pages.map((page) => page.box));
    place.replaceChildren(this.#column);
    const observer = new IntersectionObserver(
      (entries) => {
        this.#onSight(entries);
      },
      // The document as the root: in a frame of its own origin the default root (the
      // top-level viewport) is out of reach, and the margin would count for nothing.
      { root: document, rootMargin: '100% 0px' },
    );
    this.#layOut();
    for (const page of this.#pages) observer.observe(page.box);
    this.#follower = new PageFollower(() => this.#pages.map((page) => page.box), report);
    let resizing: ReturnType<typeof setTimeout> | undefined;
    window.addEventListener('resize', () => {
      clearTimeout(resizing);
      resizing = setTimeout(() => {
        this.#layOut();
      }, 150);
    });
  }

  /** The scale at zoom 1: the first page as wide as the frame allows, at most its own size. */
  #scale(): number {
    const room = Math.max(1, window.innerWidth - 2 * GUTTER);
    const width = this.#pages[0]?.size.width ?? room;
    return Math.min(1, room / width) * this.#zoom;
  }

  #layOut(): void {
    const scale = this.#scale();
    this.#column.style.setProperty('--hwp-scale', String(scale));
    for (const page of this.#pages) {
      page.box.style.width = `${Math.floor(page.size.width * scale)}px`;
      page.box.style.height = `${Math.floor(page.size.height * scale)}px`;
    }
  }

  #draw(page: Page): void {
    page.draw(this.#client).catch(() => {
      // A page that cannot be drawn, or was given up as it left the view, stays blank. But a
      // worker that was stopped (too slow, or failed on its own) draws no page any more.
      const stopped = this.#client.closed;
      if (stopped instanceof SlowRenderer) this.#report.failed('slow');
      else if (stopped instanceof WorkerFailed) this.#report.failed('renderer');
    });
  }

  #onSight(entries: IntersectionObserverEntry[]): void {
    for (const entry of entries) {
      const page = this.#pages[Number((entry.target as HTMLElement).dataset.page) - 1];
      if (!page) continue;
      if (entry.isIntersecting) this.#draw(page);
      else page.clear();
    }
  }

  /**
   * Draws the first page, the one in view when the document opens; rejects
   * when it cannot be drawn (one too large to draw shows that it is).
   */
  drawFirst(): Promise<void> {
    return this.#pages[0]?.draw(this.#client) ?? Promise.resolve();
  }

  goTo(page: number): void {
    const box = this.#pages[page - 1]?.box;
    if (!box) return;
    box.scrollIntoView({ block: 'start' });
    this.#follower.pin(page);
  }

  zoom(zoom: number): void {
    const scroller = document.scrollingElement;
    const ratio = scroller ? scroller.scrollTop / Math.max(1, scroller.scrollHeight) : 0;
    this.#zoom = zoom;
    this.#layOut();
    if (scroller) scroller.scrollTop = ratio * scroller.scrollHeight;
  }
}
