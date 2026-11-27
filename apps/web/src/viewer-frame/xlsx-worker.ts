/**
 * The spreadsheet worker: reads a workbook (`xlsx-read.ts`) off the frame's
 * main thread, so that the viewer's time limit and its "Close" hold while
 * it works, and answers once, moving the cells' arrays rather than copying
 * them. The frame ends the worker once it has answered, and the viewer
 * throws the whole frame away (and the worker with it) when the reader,
 * asked at the time limit, downloads the file instead, or the viewer closes. Built into the frame's chunk and started from a
 * `blob:`, as the Word worker is (`?worker&inline`).
 */
import { transfers, type FromXlsxWorker, type ToXlsxWorker } from './xlsx-protocol.ts';
import { failureOf, readWorkbook } from './xlsx-read.ts';

const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<ToXlsxWorker>) => void) | null;
  postMessage(message: unknown, transfer: Transferable[]): void;
};

scope.onmessage = (event) => {
  void readWorkbook(event.data.bytes).then(
    (workbook) => {
      const answer: FromXlsxWorker = { kind: 'read', workbook };
      scope.postMessage(answer, transfers(workbook));
    },
    (error: unknown) => {
      scope.postMessage(failureOf(error), []);
    },
  );
};
