/**
 * The PowerPoint worker: reads a presentation's parts and holds them to the
 * ceilings (`pptx-read.ts`) off the frame's main thread, so that the
 * viewer's time limit and its "Close" hold while it works. The frame ends
 * the worker once it has answered, and the viewer throws the whole frame
 * away (and the worker with it) when the reader, asked at the time limit,
 * downloads the file instead, or the viewer closes.
 *
 * Like the Word worker, this is built into the frame's chunk and started
 * from a `blob:` (`?worker&inline`). The pictures' bytes go to the frame
 * without a copy.
 */
import type { FromPptxWorker, ToPptxWorker } from './pptx-protocol.ts';
import { readPresentation } from './pptx-read.ts';

const scope = globalThis as unknown as {
  onmessage: ((event: MessageEvent<ToPptxWorker>) => void) | null;
  postMessage(message: unknown, transfer: Transferable[]): void;
};

scope.onmessage = (event) => {
  void readPresentation(event.data.bytes)
    .catch((): FromPptxWorker => ({ kind: 'failed', reason: 'renderer' }))
    .then((answer) => {
      const pictures =
        answer.kind === 'read'
          ? [...new Set([...answer.parts.media.values()].map((each) => each.buffer))]
          : [];
      scope.postMessage(answer, pictures);
    });
};
