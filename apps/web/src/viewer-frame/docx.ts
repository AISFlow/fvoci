/**
 * Word documents (DOCX), with docx-preview.
 *
 * docx-preview turns the document into HTML: a `section` per page, where
 * the document breaks them and where Word last broke them when it saved. Parts it would draw as raw HTML
 * (`altChunk`), comments and embedded fonts are left out. Pictures become
 * `blob:` addresses, which the frame's policy allows; anything the document
 * links to on the network stays undrawn.
 *
 * docx-preview reads and draws the document in a worker (`docx-worker.ts`),
 * on a DOM of its own with ceilings, and hands back the steps that build
 * what it drew; here they are built a slice at a time (`docx-build.ts`). So
 * the frame's main thread, which the app shares, stays free while the
 * document is read and drawn, and is given back between slices while it is
 * built: the viewer's time limit (which asks the reader whether to wait on)
 * and its "Close" hold (closing, or downloading instead, throws the frame
 * away, and the worker with it). The archive the worker gets is the one the
 * frame wrote of only the parts it counted (`zip-guard.ts`); a document that
 * would draw past the worker's ceilings is refused as too large.
 *
 * A document names its fonts ("맑은 고딕", "Calibri") and most readers have
 * few of them. Every font a document names is followed by the app's own, so
 * that Korean text falls back to Noto Sans KR rather than to whatever the
 * browser picks.
 */
import { BrokenSteps, build, measureShapes } from './docx-build.ts';
import { OPTIONS } from './docx-options.ts';
import { readFromDocxWorker, type FromDocxWorker, type ToDocxWorker } from './docx-protocol.ts';
import DocxWorker from './docx-worker.ts?worker&inline';
import {
  PageFollower,
  TooLargeFile,
  UnreadableFile,
  type Drawn,
  type Render,
  type Report,
} from './renderer.ts';

/** What every font family of the document falls back to: the app's families, as the frame's style sheet has them. */
const FALLBACK = '"Noto Sans KR", sans-serif';
/** The room around the pages, in CSS pixels, at each side. */
const GUTTER = 16;

function withFallback(family: string): string {
  return family === '' || family.includes('Noto Sans KR') ? family : `${family}, ${FALLBACK}`;
}

/** Puts the fallback after each font family the rendered document names, in its style sheets and its elements. */
function fallBack(styles: HTMLElement, body: HTMLElement): void {
  for (const element of styles.querySelectorAll('style')) {
    for (const rule of element.sheet?.cssRules ?? []) {
      if (rule instanceof CSSStyleRule && rule.style.fontFamily) {
        rule.style.fontFamily = withFallback(rule.style.fontFamily);
      }
    }
  }
  for (const element of body.querySelectorAll<HTMLElement>('[style*="font-family"]')) {
    element.style.fontFamily = withFallback(element.style.fontFamily);
  }
}

class DocxView implements Drawn {
  readonly #place: HTMLElement;
  readonly #width: number;
  #zoom = 1;

  readonly #follower: PageFollower;

  constructor(place: HTMLElement, report: Report) {
    this.#place = place;
    const first = place.querySelector('section.docx');
    this.#width = first instanceof HTMLElement ? first.offsetWidth : 816;
    this.#layOut();
    this.#follower = new PageFollower(() => this.pages, report);
    let resizing: ReturnType<typeof setTimeout> | undefined;
    window.addEventListener('resize', () => {
      clearTimeout(resizing);
      resizing = setTimeout(() => {
        this.#layOut();
      }, 150);
    });
  }

  get pages(): Element[] {
    return [...this.#place.querySelectorAll('section.docx')];
  }

  #layOut(): void {
    const room = Math.max(1, window.innerWidth - 2 * GUTTER);
    this.#place.style.zoom = String(Math.min(1, room / this.#width) * this.#zoom);
  }

  goTo(page: number): void {
    const section = this.pages[page - 1];
    if (!section) return;
    section.scrollIntoView({ block: 'start' });
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

/** Asks the Word worker to draw the file; its one answer, or a renderer's failure. */
async function drawInWorker(bytes: ArrayBuffer, token: string): Promise<FromDocxWorker> {
  const worker = new DocxWorker();
  try {
    return await new Promise<FromDocxWorker>((resolve) => {
      const failed = (): void => {
        resolve({ kind: 'failed', reason: 'renderer' });
      };
      worker.onmessage = (event: MessageEvent) => {
        resolve(readFromDocxWorker(event.data) ?? { kind: 'failed', reason: 'renderer' });
      };
      worker.onerror = failed;
      worker.onmessageerror = failed;
      const message: ToDocxWorker = { bytes, token };
      worker.postMessage(message, [bytes]);
    });
  } finally {
    // Done with this file: whatever the worker still holds goes.
    worker.terminate();
  }
}

/** A token no document can know: what the keys of its pictures start with. */
function newToken(): string {
  const random = crypto.getRandomValues(new Uint8Array(16));
  return `fvoci-picture-${Array.from(random, (byte) => byte.toString(16).padStart(2, '0')).join('')}`;
}

const FAILURES = {
  unreadable: () => new UnreadableFile(),
  'too-large': () => new TooLargeFile(),
  renderer: () => new Error('the Word worker failed'),
};

/**
 * In development the worker is not built into the chunk: the document is
 * drawn here, with the page's own DOM, and without the worker's ceilings.
 */
async function drawHere(bytes: ArrayBuffer, styles: HTMLElement, body: HTMLElement): Promise<void> {
  const { parseAsync, renderDocument } = await import('docx-preview');
  try {
    const nodes = await renderDocument(await parseAsync(bytes, OPTIONS), OPTIONS);
    for (const node of nodes) (node.nodeName === 'STYLE' ? styles : body).append(node);
  } catch {
    throw new UnreadableFile();
  }
}

/** Builds what the worker drew into the two holders, with the pictures' addresses made here. */
async function drawnByWorker(
  bytes: ArrayBuffer,
  styles: HTMLElement,
  body: HTMLElement,
): Promise<void> {
  const token = newToken();
  const answer = await drawInWorker(bytes, token);
  if (answer.kind === 'failed') throw FAILURES[answer.reason]();
  // The addresses are made here, so that they last as long as the frame.
  const addresses = new Map(answer.images.map(([key, blob]) => [key, URL.createObjectURL(blob)]));
  const keys = new RegExp(`${token}-\\d+`, 'g');
  const fill = (value: string): string =>
    value.includes(token) ? value.replace(keys, (key) => addresses.get(key) ?? '') : value;
  try {
    await build(answer.styles, styles, fill);
    await build(answer.body, body, fill);
  } catch (error) {
    throw error instanceof BrokenSteps ? new Error('the Word worker answered out of shape') : error;
  }
}

export const render: Render = async (bytes, place, report) => {
  const styles = document.createElement('div');
  const body = document.createElement('div');
  body.className = 'docx-body';
  await (import.meta.env.DEV ? drawHere : drawnByWorker)(bytes, styles, body);
  // Into the page once, built: laid out once.
  place.replaceChildren(styles, body);
  measureShapes(body);
  fallBack(styles, body);
  const view = new DocxView(body, report);
  report.shown(Math.max(1, view.pages.length));
  return view;
};
