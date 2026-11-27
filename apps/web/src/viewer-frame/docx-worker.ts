/**
 * The Word worker: docx-preview reads and draws a Word document here, off
 * the frame's main thread, so that the viewer's time limit and its "Close"
 * hold while it works. The frame ends the worker once it has answered, and
 * the viewer throws the whole frame away (and the worker with it) when the
 * reader, asked at the time limit, downloads the file instead, or the viewer
 * closes.
 *
 * A worker has no `document` and no `DOMParser`: `worker-scope.ts` puts
 * the worker's own on the global object, with their ceilings. Like pdf.js's
 * worker, this is built into the frame's chunk and started from a `blob:`
 * (`?worker&inline`).
 */
import { draw } from './docx-draw.ts';
import type { FromDocxWorker, ToDocxWorker } from './docx-protocol.ts';
import { install } from './worker-scope.ts';

const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<ToDocxWorker>) => void) | null;
  postMessage(message: unknown): void;
};

scope.onmessage = (event) => {
  const { bytes, token } = event.data;
  const { drawing } = install(token);
  void draw(bytes, drawing)
    // Anything not caught where it is drawn: the renderer's failure, at once rather than at the time limit.
    .catch((): FromDocxWorker => ({ kind: 'failed', reason: 'renderer' }))
    .then((answer) => {
      scope.postMessage(answer);
    });
};
