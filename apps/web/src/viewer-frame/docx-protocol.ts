/**
 * The messages between the viewer's frame and its Word worker
 * (`docx-worker.ts`): the frame hands over the file's bytes and a token, the
 * worker answers once, with the drawn document or why it could not be drawn.
 * The frame reads an answer only in these shapes; the steps of a drawn
 * document are checked as they are built (`docx-build.ts`). A module without
 * imports, so the frame's chunk takes it without the worker's code.
 */

/** The steps that build a tree: an element opens, then its children, then it closes. */
export const OPEN = 1;
export const CLOSE = 0;
export const TEXT = 3;
/** One step's parts: `OPEN, tag, namespace, attribute count, name, value, …`, `TEXT, text` or `CLOSE`. */
export type Step = string | number;

/** The namespaces an element may be in, by their place in the steps. */
export const NAMESPACES = [
  'http://www.w3.org/1999/xhtml',
  'http://www.w3.org/2000/svg',
  'http://www.w3.org/1998/Math/MathML',
] as const;

/** What marks a shape docx-preview would measure on the next frame: the frame measures it (`docx-build.ts`). */
export const MEASURE = 'data-fvoci-measure';

/** Why the worker could not draw the file. */
export type DocxFailure = 'unreadable' | 'too-large' | 'renderer';

export interface ToDocxWorker {
  bytes: ArrayBuffer;
  /** What each picture's key in the steps starts with: unknown to the document. */
  token: string;
}

export type FromDocxWorker =
  | {
      kind: 'drawn';
      /** The steps that build the document's style sheets (`<style>` elements). */
      styles: Step[];
      /** The steps that build the pages. */
      body: Step[];
      /** Each picture, by its key in the steps. */
      images: [string, Blob][];
    }
  | { kind: 'failed'; reason: DocxFailure };

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null;

/** The worker's answer, or undefined when it is not in one of the shapes. */
export function readFromDocxWorker(data: unknown): FromDocxWorker | undefined {
  if (!isRecord(data)) return undefined;
  if (data.kind === 'failed') {
    const reason = data.reason;
    return reason === 'unreadable' || reason === 'too-large' || reason === 'renderer'
      ? { kind: 'failed', reason }
      : undefined;
  }
  // The steps' parts are checked as they are built (`docx-build.ts`).
  if (data.kind !== 'drawn' || !Array.isArray(data.styles) || !Array.isArray(data.body)) {
    return undefined;
  }
  if (!Array.isArray(data.images)) return undefined;
  const images: [string, Blob][] = [];
  for (const image of data.images as unknown[]) {
    if (!Array.isArray(image) || typeof image[0] !== 'string' || !(image[1] instanceof Blob)) {
      return undefined;
    }
    images.push([image[0], image[1]]);
  }
  return {
    kind: 'drawn',
    styles: data.styles as Step[],
    body: data.body as Step[],
    images,
  };
}
