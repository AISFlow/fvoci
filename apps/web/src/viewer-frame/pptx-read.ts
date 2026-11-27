/**
 * What the PowerPoint worker (`pptx-worker.ts`) does with a file: checks and
 * repacks the ZIP archive (`zip-guard.ts`), unpacks the repacked parts
 * (`pptx-unpack.ts`), sorts and measures them (`pptx-parts.ts`), holds them to
 * the ceilings (`pptx-ceilings.ts`) and answers with the parts the frame's
 * renderer is to draw. In development, where the worker is not built, the
 * frame does the same.
 */
import { checkCeilings } from './pptx-ceilings.ts';
import { sortParts } from './pptx-parts.ts';
import type { FromPptxWorker, PptxFailure } from './pptx-protocol.ts';
import { unpack } from './pptx-unpack.ts';
import { UnreadableFile } from './renderer.ts';
import { TooMuch } from './worker-xml.ts';
import { repack } from './zip-guard.ts';

function failure(error: unknown): FromPptxWorker {
  // A string or a call stack past the engine's limits is a presentation too large too.
  const reason: PptxFailure =
    error instanceof TooMuch || error instanceof RangeError
      ? 'too-large'
      : error instanceof UnreadableFile
        ? 'unreadable'
        : 'renderer';
  return { kind: 'failed', reason };
}

/** Reads the archive into the parts the renderer takes, or says why not. */
export async function readPresentation(bytes: ArrayBuffer): Promise<FromPptxWorker> {
  try {
    const sorted = sortParts(await unpack(await repack(bytes)));
    // Not a presentation the renderer can draw a slide of.
    if (sorted.parts.presentation === '' || sorted.parts.slides.size === 0) {
      throw new UnreadableFile();
    }
    const { characters, slides } = checkCeilings(sorted);
    const pixels = new Map([...slides].map(([slide, count]) => [slide, count.pixels]));
    const hidden = [...sorted.parts.slides.keys()].filter(
      (slide) => sorted.measures.get(slide)?.hidden === true,
    );
    return { kind: 'read', parts: sorted.parts, characters, pixels, hidden };
  } catch (error) {
    return failure(error);
  }
}
