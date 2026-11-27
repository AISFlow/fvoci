/**
 * Draws a Word file as the Word worker does (`src/viewer-frame/docx-worker.ts`):
 * its globals (`worker-scope.ts`) put on Node's or jsdom's global object for
 * the drawing, then taken off. And a reading of the steps it answers with,
 * for the tests to look into.
 */
import { draw } from '../../src/viewer-frame/docx-draw.ts';
import {
  CLOSE,
  OPEN,
  TEXT,
  type FromDocxWorker,
  type Step,
} from '../../src/viewer-frame/docx-protocol.ts';
import { install } from '../../src/viewer-frame/worker-scope.ts';

/** Draws a Word file as the worker does, with `token` before each picture's key. */
export async function drawAsWorker(file: Buffer, token = 'TOKEN'): Promise<FromDocxWorker> {
  const { drawing, restore } = install(token);
  try {
    return await draw(Uint8Array.from(file).buffer, drawing);
  } finally {
    restore();
  }
}

/** The steps as markup, for tests to search: `<tag name="value">`, text as it is, `</tag>`. */
export function stepsAsMarkup(steps: readonly Step[]): string {
  const parts: string[] = [];
  const open: string[] = [];
  for (let at = 0; at < steps.length;) {
    const kind = steps[at++];
    if (kind === TEXT) parts.push(String(steps[at++]));
    else if (kind === CLOSE) parts.push(`</${open.pop() ?? ''}>`);
    else if (kind === OPEN) {
      const tag = String(steps[at++]);
      at += 1;
      const count = Number(steps[at++]);
      let attributes = '';
      for (let each = 0; each < count; each += 1) {
        attributes += ` ${String(steps[at++])}="${String(steps[at++])}"`;
      }
      parts.push(`<${tag}${attributes}>`);
      open.push(tag);
    }
  }
  return parts.join('');
}
