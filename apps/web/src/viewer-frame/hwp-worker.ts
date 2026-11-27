/**
 * The worker of the HWP renderer, where HWP and HWPX documents are read and
 * drawn (hwp-document.ts): nothing of a document is parsed in the frame's
 * own thread.
 *
 * Built as a classic script whose text hwp.ts carries and starts from a
 * `blob:` (`?worker&inline`), as pdf.js's worker is (pdf-worker.ts): the
 * frame's origin is opaque, and its policy holds here too, so the worker
 * reaches no network. The renderer, rhwp, is WebAssembly; its bytes come
 * with the `open` message, as the frame has them from its own bundle.
 *
 * When rhwp traps, the error is reported as the worker's own: the frame sees
 * the worker fail, stops it and says the renderer failed (worker-client.ts).
 */
import { HwpReader } from './hwp-document.ts';
import type { FromWorker, ToWorker } from './hwp-messages.ts';

/** The worker's own scope, as much of it as this uses. */
const scope = self as unknown as {
  postMessage(message: FromWorker, transfer?: Transferable[]): void;
  reportError(error: unknown): void;
  addEventListener(type: 'message', listener: (event: MessageEvent<ToWorker>) => void): void;
};

const reader = new HwpReader();

scope.addEventListener('message', (event) => {
  void reader.answer(event.data).then(
    (answer) => {
      // Checked bytes are handed over, not copied.
      scope.postMessage(answer, answer.kind === 'checked' ? [answer.bytes] : []);
    },
    (error: unknown) => {
      scope.reportError(error);
    },
  );
});
