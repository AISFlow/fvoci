/**
 * What a renderer is to the frame: something that draws a file's bytes into
 * a place and then follows the viewer's commands.
 */
import type { Failure, Labels, SheetSummary } from '$lib/file-viewer/protocol.ts';

/** What a renderer tells the viewer around it. */
export interface Report {
  /** A workbook's sheets, which are its pages: told before it is shown. */
  sheets(sheets: SheetSummary[]): void;
  /** A deck's hidden slides, by number, in order: told before it is shown, when it has any. */
  hidden(pages: number[]): void;
  /** The document is drawn as far as the reader can see it, and has this many pages. */
  shown(pages: number): void;
  /** The page most in view is now this one (from 1). */
  page(page: number): void;
  /** The document can no longer be shown, after it was: its renderer failed or took too long. */
  failed(reason: Failure): void;
  /** The reader asked for the file to download, from a page the renderer did not draw. */
  download(): void;
}

export interface Drawn {
  /** Brings a page (from 1) into view. */
  goTo(page: number): void;
  /** Draws at this zoom, 1 being the document's own size. */
  zoom(zoom: number): void;
  /** Shows a deck's hidden slides in place, or leaves them out, keeping the reader's place. */
  showHidden?(shown: boolean): void;
}

/** Thrown when the file is a kind the renderer reads, but locked with a password. */
export class LockedFile extends Error {}

/** Thrown when the bytes are not a readable file of the kind. */
export class UnreadableFile extends Error {}

/** Thrown when the file would make more than the viewer takes on (its ceilings on what is read and drawn). */
export class TooLargeFile extends Error {}

/** Thrown when the renderer took longer than the frame gives it, and was stopped. */
export class SlowRenderer extends Error {}

/** Draws a file into `place`; `labels` are the words it may show of its own, in the app's language. */
export type Render = (
  bytes: ArrayBuffer,
  place: HTMLElement,
  report: Report,
  labels: Labels,
) => Promise<Drawn>;

/** How long after a page is brought into view the scrolling it caused is let be, in milliseconds. */
const PIN_SETTLES = 150;

/**
 * Follows which page is in view while the document scrolls, and tells the
 * viewer when that changes. The page in view is the one across the line a
 * quarter of the way down the frame: a page brought into view by its top
 * (`pin`) is the page in view, however small the pages are drawn. A page's
 * number is its place among `pages` unless `numberOf` says otherwise (a
 * deck's slides keep theirs with the hidden ones left out).
 */
export class PageFollower {
  readonly #pages: () => readonly Element[];
  readonly #report: Report;
  readonly #numberOf: (page: Element, index: number) => number;
  #current = 0;
  #frame = 0;
  /** When a page was last brought into view: the scroll that follows does not move the count. */
  #pinnedAt = -Infinity;

  constructor(
    pages: () => readonly Element[],
    report: Report,
    numberOf: (page: Element, index: number) => number = (_, index) => index + 1,
  ) {
    this.#pages = pages;
    this.#report = report;
    this.#numberOf = numberOf;
    const onScroll = (): void => {
      if (this.#frame !== 0) return;
      this.#frame = requestAnimationFrame(() => {
        this.#measure();
      });
    };
    window.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', onScroll, { passive: true });
    this.#measure();
  }

  /** The page in view, as last told; 0 before any. */
  get current(): number {
    return this.#current;
  }

  /** Takes `page` (from 1) as the page in view, and tells it even when it was: it was just brought to the top. */
  pin(page: number): void {
    this.#pinnedAt = performance.now();
    this.#current = page;
    this.#report.page(page);
  }

  #measure(): void {
    this.#frame = 0;
    if (performance.now() - this.#pinnedAt < PIN_SETTLES) return;
    const line = window.innerHeight / 4;
    let best = 1;
    let bestDistance = Infinity;
    this.#pages().forEach((page, index) => {
      const box = page.getBoundingClientRect();
      const distance =
        box.top <= line && box.bottom >= line
          ? 0
          : Math.min(Math.abs(box.top - line), Math.abs(box.bottom - line));
      if (distance < bestDistance) {
        bestDistance = distance;
        best = this.#numberOf(page, index);
      }
    });
    this.#tell(best);
  }

  #tell(page: number): void {
    if (page === this.#current) return;
    this.#current = page;
    this.#report.page(page);
  }
}
