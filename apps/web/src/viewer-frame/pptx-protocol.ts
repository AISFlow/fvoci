/**
 * The messages between the viewer's frame and its PowerPoint worker
 * (`pptx-worker.ts`): the frame hands over the file's bytes, the worker
 * answers once, with the parts of the presentation it read and checked, or
 * why it could not. The parts are in the shape the renderer takes them in
 * (its `PptxFiles`: each part's text, and the pictures' bytes); the frame
 * reads an answer only in that shape. A module without imports, so the
 * frame's chunk takes it without the worker's code.
 */
/** Why the worker could not read the file. */
export type PptxFailure = 'unreadable' | 'too-large' | 'renderer';

/**
 * The parts of a presentation the renderer is given (its `PptxFiles`, of
 * which this is a part): no fonts, and the maps of charts empty
 * (`pptx-parts.ts`).
 */
export interface PptxParts {
  contentTypes: string;
  presentation: string;
  presentationRels: string;
  tableStyles?: string;
  slides: Map<string, string>;
  slideRels: Map<string, string>;
  slideLayouts: Map<string, string>;
  slideLayoutRels: Map<string, string>;
  slideMasters: Map<string, string>;
  slideMasterRels: Map<string, string>;
  themes: Map<string, string>;
  media: Map<string, Uint8Array>;
  charts: Map<string, string>;
  chartStyles: Map<string, string>;
  chartColors: Map<string, string>;
  diagramDrawings: Map<string, string>;
}

export interface ToPptxWorker {
  bytes: ArrayBuffer;
}

export type FromPptxWorker =
  | {
      kind: 'read';
      parts: PptxParts;
      /** Every character the slides' text has, once: the fonts the frame loads before it draws. */
      characters: string;
      /** The pixels of the pictures each slide shows, by the slide's name: what drawing it holds. */
      pixels: Map<string, number>;
      /** The slides hidden from a show (`p:sld@show` false), by name. */
      hidden: string[];
    }
  | { kind: 'failed'; reason: PptxFailure };

const TEXT_FIELDS = ['contentTypes', 'presentation', 'presentationRels'] as const;
const TEXT_MAPS = [
  'slides',
  'slideRels',
  'slideLayouts',
  'slideLayoutRels',
  'slideMasters',
  'slideMasterRels',
  'themes',
  'charts',
  'chartStyles',
  'chartColors',
  'diagramDrawings',
] as const;

function mapOf<T>(
  value: unknown,
  isValue: (each: unknown) => each is T,
): Map<string, T> | undefined {
  if (!(value instanceof Map)) return undefined;
  for (const [key, each] of value as Map<unknown, unknown>) {
    if (typeof key !== 'string' || !isValue(each)) return undefined;
  }
  return value as Map<string, T>;
}

const isString = (value: unknown): value is string => typeof value === 'string';
const isBytes = (value: unknown): value is Uint8Array => value instanceof Uint8Array;
const isCount = (value: unknown): value is number =>
  typeof value === 'number' && Number.isFinite(value) && value >= 0;

/** The parts, when `value` has every field in its shape and nothing else of note; undefined otherwise. */
function readParts(value: unknown): PptxParts | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const fields = value as Record<string, unknown>;
  const parts: Record<string, unknown> = {};
  for (const name of TEXT_FIELDS) {
    if (!isString(fields[name])) return undefined;
    parts[name] = fields[name];
  }
  if (fields.tableStyles !== undefined && !isString(fields.tableStyles)) return undefined;
  parts.tableStyles = fields.tableStyles;
  for (const name of TEXT_MAPS) {
    const map = mapOf(fields[name], isString);
    if (!map) return undefined;
    parts[name] = map;
  }
  const media = mapOf(fields.media, isBytes);
  if (!media) return undefined;
  parts.media = media;
  return parts as unknown as PptxParts;
}

/** The worker's answer, or undefined when it is not in one of the shapes. */
export function readFromPptxWorker(data: unknown): FromPptxWorker | undefined {
  if (typeof data !== 'object' || data === null) return undefined;
  const message = data as Record<string, unknown>;
  if (message.kind === 'failed') {
    const reason = message.reason;
    return reason === 'unreadable' || reason === 'too-large' || reason === 'renderer'
      ? { kind: 'failed', reason }
      : undefined;
  }
  if (message.kind !== 'read' || !isString(message.characters)) return undefined;
  const { hidden } = message;
  if (!Array.isArray(hidden) || !hidden.every(isString)) return undefined;
  const parts = readParts(message.parts);
  const pixels = mapOf(message.pixels, isCount);
  return parts && pixels
    ? { kind: 'read', parts, characters: message.characters, pixels, hidden }
    : undefined;
}
