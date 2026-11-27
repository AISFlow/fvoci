/**
 * The parts of a presentation sorted as the renderer takes them (its
 * `PptxFiles`, with the part names it reads: `parseZip` of
 * @aiden0z/pptx-renderer), in the PowerPoint worker. Each XML part is read
 * as text once, here, and the renderer gets that very text; each is measured
 * (`pptx-xml.ts`) for the worker's ceilings (`pptx-ceilings.ts`).
 *
 * Left out, so that the frame neither reads nor draws them: embedded fonts
 * (the app's fonts stand in), charts (a placeholder stands in), pictures of
 * kinds other than those `pptx-images.ts` reads, sounds and films, and
 * every relationship to something outside the file or to a web page. What
 * a slide links to within the presentation stays.
 *
 * The renderer finds parts by names and relationships it reads its own way;
 * the worker counts what it would draw only if both read them alike. So
 * nothing is left to its reading: a part whose name it would read otherwise
 * (an escape, a backslash, a query) is left out, and every relationship
 * part is written anew, each target the name of a part that is kept, as an
 * absolute name the renderer resolves to that very name, of a type it reads
 * (a chart's, whose part is left out, too: the renderer draws a box there).
 * A relationship part that gives an identifier twice is unreadable, as is
 * a slide list that would have the renderer read one slide (and its
 * relationships) twice. An identifier longer than `NAME_LENGTH`, or one
 * or a target with a tab or a line break (which a parser reads as a space
 * once written back), is left out. A picture is kept only under a name of a
 * kind the renderer hands to the browser as a picture (it reads `.emf`
 * itself, by the name, whatever the bytes).
 */
import { SaxesParser } from 'saxes';

import type { PptxParts } from './pptx-protocol.ts';
import { pixelsOf, type Pixels } from './pptx-images.ts';
import { measure, NAME_LENGTH, type PartMeasure } from './pptx-xml.ts';
import { UnreadableFile } from './renderer.ts';

const RELATIONSHIPS = 'http://schemas.openxmlformats.org/package/2006/relationships';

/** Where the types of relationships the renderer draws by are named, transitional and strict. */
const TYPE_PREFIXES = [
  'http://schemas.openxmlformats.org/officeDocument/2006/relationships/',
  'http://purl.oclc.org/ooxml/officeDocument/relationships/',
  'http://schemas.microsoft.com/office/2007/relationships/',
];
/** The types of relationships kept: what the renderer draws with. */
const TYPES = new Set([
  'slide',
  'slideLayout',
  'slideMaster',
  'theme',
  'image',
  'tableStyles',
  'diagramData',
  'diagramLayout',
  'diagramQuickStyle',
  'diagramColors',
  'diagramDrawing',
  // Charts are left out; the relationship stays, so that the renderer draws the box that says so.
  'chart',
]);

/** The maps of `PptxParts` the parts are sorted into, by the names the renderer reads. */
const SORTED = [
  [/^ppt\/slides\/_rels\/[^/]+\.xml\.rels$/, 'slideRels'],
  [/^ppt\/slides\/[^/]+\.xml$/, 'slides'],
  [/^ppt\/slideLayouts\/_rels\/[^/]+\.xml\.rels$/, 'slideLayoutRels'],
  [/^ppt\/slideLayouts\/[^/]+\.xml$/, 'slideLayouts'],
  [/^ppt\/slideMasters\/_rels\/[^/]+\.xml\.rels$/, 'slideMasterRels'],
  [/^ppt\/slideMasters\/[^/]+\.xml$/, 'slideMasters'],
  [/^ppt\/theme\/(?!themeOverride[^/]*\.xml$)[^/]+\.xml$/, 'themes'],
  [/^ppt\/diagrams\/[^/]+\.xml$/, 'diagramDrawings'],
] as const;

type SortedMap = (typeof SORTED)[number][1];

const SINGLE = {
  '[Content_Types].xml': 'contentTypes',
  'ppt/presentation.xml': 'presentation',
  'ppt/_rels/presentation.xml.rels': 'presentationRels',
  'ppt/tableStyles.xml': 'tableStyles',
} as const;

/** One relationship of a part, as its relationship part says it. */
export interface Relationship {
  id: string;
  type: string;
  target: string;
}

/** What the worker knows of the presentation once the parts are sorted. */
export interface Sorted {
  parts: PptxParts;
  /** Each XML part kept, measured, by its name. */
  measures: Map<string, PartMeasure>;
  /** Each relationship part kept, read, by its name. */
  relationships: Map<string, Relationship[]>;
  /** Each picture kept, by its name under `ppt/media/`. */
  pixels: Map<string, Pixels>;
}

const decoder = new TextDecoder('utf-8');

/** Whether the renderer reads the name as it is: no escapes, backslashes, queries, fragments, tabs or line breaks. */
export const isPlainName = (name: string): boolean => !/[%\\?#\t\n\r]/.test(name);

/** The endings of the pictures' names the renderer hands to the browser to decode (its `xm`, `Rn`). */
const PICTURE_ENDINGS = new Set(['png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp']);

/** The ending of a name, as the renderer takes it (after the last dot, wherever it is). */
const endingOf = (name: string): string => (name.split('.').pop() ?? '').toLowerCase();

/** A type the renderer draws by, of those in `TYPES`. */
function isKeptType(type: string): boolean {
  const prefix = TYPE_PREFIXES.find((each) => type.startsWith(each));
  return prefix !== undefined && TYPES.has(type.slice(prefix.length));
}

/** The part a relationship of the part in `directory` leads to, as the renderer resolves it (its `Kt`). */
export function resolveTarget(directory: string, target: string): string {
  const cut = target.search(/[?#]/);
  const path = (cut >= 0 ? target.slice(0, cut) : target).replace(/\\/g, '/');
  const decode = (segment: string): string => {
    try {
      return decodeURIComponent(segment);
    } catch {
      return segment;
    }
  };
  const segments = path.split('/').filter(Boolean).map(decode);
  const resolved = path.startsWith('/') ? [] : directory.split('/').filter(Boolean);
  for (const segment of segments) {
    if (segment === '..') resolved.pop();
    else if (segment !== '.') resolved.push(segment);
  }
  return resolved.join('/');
}

/** The directory of the part a relationship part is of: `ppt/slides` for `ppt/slides/_rels/slide1.xml.rels`. */
const directoryOf = (relationshipPart: string): string =>
  relationshipPart.replace(/(^|\/)_rels\/[^/]*$/, '');

/** The relationships as read from a relationship part, before they are judged. */
function parseRelationships(text: string): (Relationship & { outside: boolean })[] {
  const read: (Relationship & { outside: boolean })[] = [];
  const parser = new SaxesParser({ xmlns: true, position: false });
  parser.on('doctype', () => {
    throw new UnreadableFile();
  });
  parser.on('opentag', (tag) => {
    if (tag.local !== 'Relationship' || tag.uri !== RELATIONSHIPS) return;
    const value = (name: string): string => tag.attributes[name]?.value ?? '';
    const outside = value('TargetMode').trim().toLowerCase() === 'external';
    read.push({ id: value('Id'), type: value('Type'), target: value('Target'), outside });
  });
  parser.on('error', () => {
    throw new UnreadableFile();
  });
  parser.write(text).close();
  return read;
}

/**
 * The relationships kept of a relationship part: to a part that is kept
 * (`kept`), of a type kept, each identifier once; the target written as the
 * absolute name of the part.
 */
function readRelationships(
  name: string,
  text: string,
  kept: (part: string) => boolean,
): Relationship[] {
  const directory = directoryOf(name);
  const byId = new Map<string, Relationship>();
  const seen = new Set<string>();
  for (const { id, type, target, outside } of parseRelationships(text)) {
    if (outside || !isKeptType(type)) continue;
    if (id.length > NAME_LENGTH || /[\t\n\r]/.test(id + target)) continue;
    // The renderer would keep the first's place and the last's target: no file PowerPoint writes.
    if (seen.has(id)) throw new UnreadableFile();
    seen.add(id);
    const part = resolveTarget(directory, target);
    const leads = kept(part) || type.endsWith('/chart');
    if (id !== '' && isPlainName(part) && leads) byId.set(id, { id, type, target: `/${part}` });
  }
  return [...byId.values()];
}

const escape = (value: string): string =>
  value.replace(
    /[&<>"\t\n\r]/g,
    (character) =>
      ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        '\t': '&#9;',
        '\n': '&#10;',
        '\r': '&#13;',
      })[character] ?? '',
  );

/** A relationship part of only these relationships. */
export function writeRelationships(relationships: readonly Relationship[]): string {
  const each = relationships.map(
    ({ id, type, target }) =>
      `<Relationship Id="${escape(id)}" Type="${escape(type)}" Target="${escape(target)}"/>`,
  );
  return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="${RELATIONSHIPS}">${each.join('')}</Relationships>`;
}

function emptyParts(): PptxParts {
  const map = (): Map<string, string> => new Map();
  return {
    contentTypes: '',
    presentation: '',
    presentationRels: '',
    slides: map(),
    slideRels: map(),
    slideLayouts: map(),
    slideLayoutRels: map(),
    slideMasters: map(),
    slideMasterRels: map(),
    themes: map(),
    media: new Map(),
    charts: map(),
    chartStyles: map(),
    chartColors: map(),
    diagramDrawings: map(),
  };
}

const isRelationshipPart = (name: string): boolean => name.endsWith('.rels');

/** Where a part goes: one of the single parts, one of the maps, or nowhere. */
function placeOf(
  name: string,
): { single: (typeof SINGLE)[keyof typeof SINGLE] } | { map: SortedMap } | undefined {
  if (Object.hasOwn(SINGLE, name)) return { single: SINGLE[name as keyof typeof SINGLE] };
  const map = SORTED.find(([pattern]) => pattern.test(name))?.[1];
  return map ? { map } : undefined;
}

function place(parts: PptxParts, name: string, text: string): void {
  const where = placeOf(name);
  if (where && 'single' in where) Object.assign(parts, { [where.single]: text });
  else if (where) parts[where.map].set(name, text);
}

/**
 * Sorts the unpacked parts; throws `UnreadableFile` when an XML part kept
 * is not one the frame reads alike. The relationship parts come last, as
 * they keep only relationships to parts kept.
 */
export function sortParts(unpacked: ReadonlyMap<string, Uint8Array>): Sorted {
  const sorted: Sorted = {
    parts: emptyParts(),
    measures: new Map(),
    relationships: new Map(),
    pixels: new Map(),
  };
  const { parts } = sorted;
  const kept = new Set<string>();
  const relationshipParts: [string, Uint8Array][] = [];
  for (const [name, bytes] of unpacked) {
    if (!isPlainName(name)) continue;
    if (name.startsWith('ppt/media/')) {
      if (!PICTURE_ENDINGS.has(endingOf(name))) continue;
      const pixels = pixelsOf(bytes);
      if (!pixels) continue;
      parts.media.set(name, bytes);
      sorted.pixels.set(name, pixels);
      kept.add(name);
    } else if (placeOf(name) && isRelationshipPart(name)) {
      relationshipParts.push([name, bytes]);
    } else if (placeOf(name)) {
      const text = decoder.decode(bytes);
      sorted.measures.set(name, measure(text));
      place(parts, name, text);
      kept.add(name);
    }
  }
  for (const [name, bytes] of relationshipParts) {
    const relationships = readRelationships(name, decoder.decode(bytes), (part) => kept.has(part));
    sorted.relationships.set(name, relationships);
    const text = writeRelationships(relationships);
    sorted.measures.set(name, measure(text));
    place(parts, name, text);
  }
  if (listsASlideTwice(sorted)) throw new UnreadableFile();
  return sorted;
}

/**
 * Whether the renderer may read one slide (and its relationships) twice: it
 * reads a slide once for each entry of the slide list, an entry by one of
 * its attributes named `id`, or, when no entry leads to a slide, once for
 * each relationship of the presentation to it.
 */
function listsASlideTwice(sorted: Sorted): boolean {
  const refs = sorted.measures.get('ppt/presentation.xml')?.slideRefs ?? [];
  const relationships = sorted.relationships.get('ppt/_rels/presentation.xml.rels') ?? [];
  const links = new Map(relationships.map((each) => [each.id, resolveTarget('ppt', each.target)]));
  const linked = [...links.values()].filter((part) => sorted.parts.slides.has(part));
  if (new Set(linked).size < linked.length) return true;
  const named = new Set<string>();
  for (const ids of refs) {
    const slides = new Set(
      ids.flatMap((id) => links.get(id) ?? []).filter((part) => sorted.parts.slides.has(part)),
    );
    for (const slide of slides) {
      if (named.has(slide)) return true;
      named.add(slide);
    }
  }
  return false;
}
