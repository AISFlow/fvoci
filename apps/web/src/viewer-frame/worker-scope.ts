/**
 * What the Word worker (`docx-worker.ts`) puts on its global object before
 * docx-preview runs: the DOM of `worker-html.ts` and `worker-xml.ts`, with
 * their ceilings, a `URL.createObjectURL` that keeps each picture as a blob
 * under a key, and a `requestAnimationFrame` that does nothing (docx-preview
 * asks for frames only to measure shapes, which the frame does).
 *
 * The tests put the same on Node's global object, and take it off again.
 */
import { Budget, documentFor, Node } from './worker-html.ts';
import { domParserFor, NodeCount } from './worker-xml.ts';

/** What the globals collect while a document is drawn. */
export interface Drawing {
  budget: Budget;
  /** Each picture, by its key: `token`, a dash, a number. */
  images: [string, Blob][];
}

/** Puts the worker's globals on the global object; returns what they collect, and how to put back what was there. */
export function install(token: string): { drawing: Drawing; restore: () => void } {
  const drawing: Drawing = { budget: new Budget(), images: [] };
  const { budget, images } = drawing;
  const values: Record<string, unknown> = {
    document: documentFor(budget),
    Node,
    DOMParser: domParserFor(new NodeCount()),
    // docx-preview asks for a frame only to measure a shape: there is no layout here, and
    // the frame measures what was marked when it was made (`worker-html.ts`).
    requestAnimationFrame: () => 0,
  };
  // As they were, accessors included (jsdom's `document` has only a getter).
  const before = Object.keys(values).map(
    (name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const,
  );
  for (const [name, value] of Object.entries(values)) {
    Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
  }
  const createObjectURL = URL.createObjectURL.bind(URL);
  URL.createObjectURL = (blob: Blob | MediaSource) => {
    const key = `${token}-${images.length}`;
    if (blob instanceof Blob) images.push([key, blob]);
    return key;
  };
  const restore = (): void => {
    for (const [name, descriptor] of before) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
    URL.createObjectURL = createObjectURL;
  };
  return { drawing, restore };
}
