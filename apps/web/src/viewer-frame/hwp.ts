/**
 * HWP and HWPX (the word processor documents of Korea), with rhwp.
 *
 * rhwp is WebAssembly, and it runs in a worker of its own (hwp-worker.ts):
 * the file is checked, read, laid out and drawn there, and the frame's own
 * thread only puts what the worker drew into the page (hwp-view.ts). The
 * worker has a time for each thing it is asked (opening the file, drawing a
 * page) once the document is shown; past it, or when the frame goes, it is
 * stopped (`terminate()`) and its memory with it. Until then the app keeps
 * the time, as for any file (`$lib/file-viewer/session.svelte.ts`): it asks
 * the reader whether to wait on for a document that takes long to open, and
 * the worker goes on meanwhile.
 *
 * rhwp's bytes come with the document, from the app
 * (`$lib/file-viewer/engine.ts`): the frame cannot fetch them, as it cannot
 * fetch anything. They are handed on to the worker, not copied.
 */
import HwpWorker from './hwp-worker.ts?worker&inline';
import type { HwpFormat } from './hwp-messages.ts';
import { ask, HwpView } from './hwp-view.ts';
import type { Render } from './renderer.ts';
import { WorkerClient, type WorkerLike } from './worker-client.ts';

/** How long the worker may take over one thing it is asked once the document is shown, in milliseconds. */
export const WORK_LIMIT = 30_000;

/** How the renderer starts: its worker, and how long the worker may take. */
export interface Start {
  worker(): WorkerLike;
  /** How long the worker may take over one thing once the document is shown, in milliseconds. */
  limit: number;
}

const START: Start = { worker: () => new HwpWorker(), limit: WORK_LIMIT };

/**
 * The renderer of HWP (a compound file) or HWPX (an archive), with rhwp's
 * WebAssembly as the app handed it over.
 */
export function renderer(
  format: HwpFormat,
  wasm: ArrayBuffer | null,
  start: Start = START,
): Render {
  return async (bytes, place, report, labels) => {
    if (!wasm) throw new Error('the HWP renderer was given no engine');
    const client = new WorkerClient(start.worker(), undefined);
    // The viewer takes the frame out of the page when it closes: the worker stops then too.
    window.addEventListener(
      'pagehide',
      () => {
        client.close();
      },
      { once: true },
    );
    try {
      const opened = await ask(client, { kind: 'open', format, bytes, wasm }, [bytes, wasm]);
      if (opened.kind !== 'opened') throw new Error('the HWP renderer answered out of turn');
      const view = new HwpView(client, opened.pages, place, report, labels);
      // Shown once the reader sees a page: the time to draw it counts, and a first page that
      // cannot be drawn is a failure rather than a white box (one too large to draw says so).
      await view.drawFirst();
      // A page in view asked for meanwhile has its time from here.
      client.limitEach(start.limit);
      report.shown(opened.pages.length);
      return view;
    } catch (error) {
      client.close();
      throw error;
    }
  };
}
