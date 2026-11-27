/**
 * PowerPoint presentations (PPTX), with @aiden0z/pptx-renderer.
 *
 * The renderer draws a slide as HTML and SVG, and fits text to its boxes by
 * measuring it in the page's layout: it cannot draw in a worker, as the
 * Word renderer does. So the work is split. The PowerPoint worker
 * (`pptx-worker.ts`) checks and repacks the archive of only the parts it
 * counted (`zip-guard.ts`), unpacks it, reads every part, leaves out what is
 * not drawn and holds the rest to ceilings (`pptx-ceilings.ts`): a
 * presentation past one is refused as too large before anything is drawn. Here, on the
 * frame's thread (which the app shares), the renderer builds its model of
 * the parts the worker answered with and draws the slides in view one at a
 * time, the thread given back between them (`pptx-view.ts`); the viewer's
 * time limit (which asks the reader whether to wait on) and its "Close" hold
 * throughout (closing, or downloading instead, throws the frame away, and
 * the worker with it).
 *
 * A slide hidden from a show (`p:sld@show`, read by the worker: the
 * renderer's model learns it only of a slide it draws) is left out until
 * the reader asks for the hidden slides, and then marked in place.
 *
 * Not drawn: animations and transitions (a slide is drawn as it stands),
 * films and sounds, speaker notes, embedded fonts and charts (a box says
 * where a chart was, in words the app gives). The renderer's chart library and its embedded-font
 * decoder are left out of the build (vite.config.ts). Before a slide is
 * drawn, the app's Korean font is loaded for the characters of the
 * presentation, so the renderer fits text in the font it will be shown in.
 */
import { buildPresentation, renderSlide, type PptxFiles } from '@aiden0z/pptx-renderer';

import { SlideOrder } from '$lib/file-viewer/slide-order.ts';

import { readFromPptxWorker, type FromPptxWorker, type ToPptxWorker } from './pptx-protocol.ts';
import { PptxView, type DrawSlide } from './pptx-view.ts';
import PptxWorker from './pptx-worker.ts?worker&inline';
import { TooLargeFile, UnreadableFile, type Render } from './renderer.ts';

/** Asks the PowerPoint worker to read the file; its one answer, or a renderer's failure. */
async function readInWorker(bytes: ArrayBuffer): Promise<FromPptxWorker> {
  const worker = new PptxWorker();
  try {
    return await new Promise<FromPptxWorker>((resolve) => {
      const failed = (): void => {
        resolve({ kind: 'failed', reason: 'renderer' });
      };
      worker.onmessage = (event: MessageEvent) => {
        resolve(readFromPptxWorker(event.data) ?? { kind: 'failed', reason: 'renderer' });
      };
      worker.onerror = failed;
      worker.onmessageerror = failed;
      const message: ToPptxWorker = { bytes };
      worker.postMessage(message, [bytes]);
    });
  } finally {
    // Done with this file: whatever the worker still holds goes.
    worker.terminate();
  }
}

/** In development the worker is not built into the chunk: the file is read here. */
async function readHere(bytes: ArrayBuffer): Promise<FromPptxWorker> {
  const { readPresentation } = await import('./pptx-read.ts');
  return readPresentation(bytes);
}

const FAILURES = {
  unreadable: () => new UnreadableFile(),
  'too-large': () => new TooLargeFile(),
  renderer: () => new Error('the PowerPoint worker failed'),
};

/** Loads the app's Korean font, regular and bold, for the characters the slides have. */
async function loadFonts(characters: string): Promise<void> {
  if (characters === '') return;
  await Promise.all(
    ['400', '700'].map((weight) =>
      document.fonts.load(`${weight} 16px "Noto Sans KR"`, characters).catch(() => []),
    ),
  );
}

export const render: Render = async (bytes, place, report, labels) => {
  const answer = await (import.meta.env.DEV ? readHere : readInWorker)(bytes);
  if (answer.kind === 'failed') throw FAILURES[answer.reason]();
  const files: PptxFiles = answer.parts;
  let presentation;
  try {
    presentation = buildPresentation(files, { lazySlides: true });
  } catch {
    throw new UnreadableFile();
  }
  if (presentation.slides.length === 0) throw new UnreadableFile();
  await loadFonts(answer.characters);
  const model = presentation;
  const draw: DrawSlide = (index, options) => {
    const slide = model.slides[index];
    if (!slide) throw new Error(`no slide ${index}`);
    return renderSlide(model, slide, options);
  };
  const hiddenParts = new Set(answer.hidden);
  const hidden = model.slides.flatMap((slide, index) =>
    hiddenParts.has(slide.slidePath) ? [index + 1] : [],
  );
  if (hidden.length > 0) report.hidden(hidden);
  const order = new SlideOrder(model.slides.length, hidden);
  const view = new PptxView(model, place, report, draw, labels, answer.pixels, order);
  // Shown once the reader sees a slide, so that the time to draw counts and a slide that
  // cannot be drawn is a failure rather than a white box.
  await view.drawFirst();
  report.shown(presentation.slides.length);
  return view;
};
