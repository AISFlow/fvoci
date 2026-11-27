/**
 * Workbooks (XLSX), read by our own reader and drawn as a grid.
 *
 * The workbook is read in a worker (`xlsx-worker.ts`, `xlsx-read.ts`) with
 * ceilings on what it holds, all at once: the worker answers once and is
 * ended, and the viewer throws the frame away (and the worker with it) when
 * its time is up or it closes. The archive the worker gets is the one the
 * frame wrote of only the parts it counted (`zip-guard.ts`). The answer is
 * taken only in its shapes (`xlsx-answer.ts`); the frame then draws one
 * sheet at a time (`xlsx-grid.ts`), and of it only what is in view, each
 * number through its format (`xlsx-format.ts`) as the cell comes into view.
 *
 * A page of the viewer is a sheet: the left and right arrows, the viewer's
 * buttons and its tabs go to the others; with Shift, the arrows across
 * scroll the sheet across (`main.ts`). Sheets the workbook hides are left
 * out. Charts, pictures and shapes are not drawn: the viewer says so of a
 * sheet that has them.
 */
import { readFromXlsxWorker } from './xlsx-answer.ts';
import { SheetView } from './xlsx-grid.ts';
import { Looks } from './xlsx-look.ts';
import type { FromXlsxWorker, ToXlsxWorker, Workbook } from './xlsx-protocol.ts';
import XlsxWorker from './xlsx-worker.ts?worker&inline';
import { TooLargeFile, UnreadableFile, type Drawn, type Render, type Report } from './renderer.ts';

const FAILURES = {
  unreadable: () => new UnreadableFile(),
  'too-large': () => new TooLargeFile(),
  renderer: () => new Error('the spreadsheet worker failed'),
};

/** Asks the spreadsheet worker to read the file; its one answer, or a renderer's failure. */
async function readInWorker(bytes: ArrayBuffer): Promise<Workbook> {
  const worker = new XlsxWorker();
  let answer: FromXlsxWorker;
  try {
    answer = await new Promise<FromXlsxWorker>((resolve) => {
      const failed = (): void => {
        resolve({ kind: 'failed', reason: 'renderer' });
      };
      worker.onmessage = (event: MessageEvent) => {
        resolve(readFromXlsxWorker(event.data) ?? { kind: 'failed', reason: 'renderer' });
      };
      worker.onerror = failed;
      worker.onmessageerror = failed;
      const message: ToXlsxWorker = { bytes };
      worker.postMessage(message, [bytes]);
    });
  } finally {
    // Done with this file: whatever the worker still holds goes.
    worker.terminate();
  }
  if (answer.kind === 'failed') throw FAILURES[answer.reason]();
  return answer.workbook;
}

/** In development the worker is not built into the chunk: the workbook is read here, with the same reader and ceilings. */
async function readHere(bytes: ArrayBuffer): Promise<Workbook> {
  const { failureOf, readWorkbook } = await import('./xlsx-read.ts');
  try {
    return await readWorkbook(bytes);
  } catch (error) {
    const failure = failureOf(error);
    throw failure.kind === 'failed' ? FAILURES[failure.reason]() : error;
  }
}

class WorkbookView implements Drawn {
  readonly #workbook: Workbook;
  readonly #looks: Looks;
  readonly #place: HTMLElement;
  readonly #report: Report;
  #view: SheetView | undefined;
  #index = -1;
  #zoom = 1;
  #frame = 0;

  constructor(workbook: Workbook, place: HTMLElement, report: Report) {
    this.#workbook = workbook;
    this.#looks = new Looks(workbook, document.documentElement.lang || 'en');
    this.#place = place;
    this.#report = report;
    const redraw = (): void => {
      if (this.#frame !== 0) return;
      this.#frame = requestAnimationFrame(() => {
        this.#frame = 0;
        this.#view?.draw();
      });
    };
    window.addEventListener('scroll', redraw, { passive: true });
    window.addEventListener('resize', redraw, { passive: true });
    this.#show(0);
  }

  #show(index: number): void {
    const sheet = this.#workbook.sheets[index];
    if (!sheet || index === this.#index) return;
    this.#index = index;
    const view = new SheetView(sheet, this.#looks);
    this.#view = view;
    this.#place.replaceChildren(view.element);
    window.scrollTo(0, 0);
    if (this.#zoom === 1) view.draw();
    else view.zoom(this.#zoom);
  }

  goTo(page: number): void {
    if (!this.#workbook.sheets[page - 1]) return;
    this.#show(page - 1);
    this.#report.page(page);
  }

  zoom(zoom: number): void {
    this.#zoom = zoom;
    this.#view?.zoom(zoom);
  }
}

export const render: Render = async (bytes, place, report) => {
  const workbook = await (import.meta.env.DEV ? readHere : readInWorker)(bytes);
  const view = new WorkbookView(workbook, place, report);
  report.sheets(workbook.sheets.map((sheet) => ({ name: sheet.name, drawings: sheet.drawings })));
  report.shown(workbook.sheets.length);
  return view;
};
