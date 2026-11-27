/**
 * What rhwp parses of an HWPX package, held to the viewer's limit on XML, in
 * the renderer's worker once the archive was counted and written anew
 * (zip-guard.ts).
 *
 * zip-guard counts a part as XML by what it holds (its first character "<"),
 * and each part once, as the Word renderer reads them. rhwp (0.9.0) reads an
 * HWPX otherwise, from the package's own list (`Contents/content.hpf`):
 *
 * - the sections are the parts the spine names (or, when it names none, every
 *   XML part of the manifest whose address says "section"), in that order,
 *   each parsed as XML whatever it starts with, as often as it is named
 *   (rhwp stops at as many as the header says there are, `secCnt`, when that
 *   is fewer; the count here is of them all, which is never less);
 * - the master pages (the manifest's XML parts whose address says
 *   "masterpage") are parsed again for each section that uses them.
 *
 * So here a spine that names one part twice stops the file (no document does
 * that), every part counts as XML but the pictures and other data under
 * `BinData/`, and the master pages count once more for every section after
 * the first. Together they are held to `MARKUP_LIMIT_BYTES`.
 *
 * The package is read as rhwp's reader reads it (hpf-markup.ts): elements by
 * their local name (`item`, `itemref`, whatever comes before the last `:`),
 * attributes by their exact name (`id`, `href`, `media-type`, `idref`) with
 * their values as written, and markup ended where that reader ends it. A tag
 * with more than 1,000 attributes stops the file.
 *
 * rhwp looks each name of the spine up by going through the items from the
 * first, so a long spine over a long manifest costs it their product: 20,000
 * of each a second, 80,000 eight. A list whose product is past
 * `LOOKUP_LIMIT` stops the file; a document's is a few thousand.
 */
import { MARKUP_LIMIT_BYTES } from '$lib/file-viewer/formats.ts';
import { zipEntries, type ZipEntry } from '$lib/file-viewer/zip-directory.ts';

import { readTags } from './hpf-markup.ts';
import { TooLargeFile, UnreadableFile } from './renderer.ts';

const PACKAGE = 'Contents/content.hpf';
const XML = 'application/xml';
const LOCAL_SIZE = 30;
const STORED = 0;

/** The most a spine's names times a manifest's items may be: what rhwp's lookups may cost it. */
export const LOOKUP_LIMIT = 100_000_000;

interface Item {
  id: string;
  href: string;
  mediaType: string;
}

/** What the package lists: its items, and the ids its spine names in order. */
interface Listed {
  items: Item[];
  spine: string[];
}

/**
 * The items and the spine of `content.hpf`'s text, from its elements as rhwp's
 * reader gives them (hpf-markup.ts): by their local name, after the last `:`,
 * with the attributes it takes, their values as written (rhwp leaves entities
 * as they are). Markup it stops at makes the file unreadable, as it does rhwp.
 */
export function listed(xml: string): Listed {
  const items: Item[] = [];
  const spine: string[] = [];
  readTags(xml, ({ name, attributes }) => {
    const local = name.slice(name.lastIndexOf(':') + 1);
    if (local === 'itemref') {
      for (const [key, value] of attributes()) if (key === 'idref') spine.push(value);
    } else if (local === 'item') {
      const found = new Map(attributes());
      const id = found.get('id') ?? '';
      const href = found.get('href') ?? '';
      if (id && href) items.push({ id, href, mediaType: found.get('media-type') ?? '' });
    }
  });
  return { items, spine };
}

/**
 * The parts rhwp reads as sections, in order, as many times as it reads each.
 * Throws `TooLargeFile` when the spine and the manifest are longer together
 * than rhwp looks up in good time (`LOOKUP_LIMIT`).
 */
export function sectionsOf({ items, spine }: Listed): string[] {
  if (spine.length * items.length > LOOKUP_LIMIT) throw new TooLargeFile();
  const isSection = (item: Item): boolean =>
    item.mediaType === XML && item.href.includes('section');
  // An id names the first item that has it, as rhwp finds it.
  const byId = new Map<string, Item>();
  for (const item of items) if (!byId.has(item.id)) byId.set(item.id, item);
  const named = spine.flatMap((idref) => {
    const item = byId.get(idref);
    return item && isSection(item) ? [item.href] : [];
  });
  if (named.length > 0) return named;
  return items
    .filter(isSection)
    .map((item) => item.href)
    .sort();
}

/** The master pages rhwp may read for each section: each address once. */
export function masterPagesOf({ items }: Listed): string[] {
  const pages = items.filter(
    (item) => item.mediaType === XML && item.href.toLowerCase().includes('masterpage'),
  );
  return [...new Set(pages.map((item) => item.href))];
}

const decoder = new TextDecoder();

/** The text of a part of an archive that zip-guard wrote (honest offsets and sizes). */
async function partText(bytes: Uint8Array, entry: ZipEntry): Promise<string> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const at = entry.offset;
  const start = at + LOCAL_SIZE + view.getUint16(at + 26, true) + view.getUint16(at + 28, true);
  const packed = bytes.slice(start, start + entry.compressedSize);
  if (entry.method === STORED) return decoder.decode(packed);
  const stream = new Blob([packed]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
  return decoder.decode(await new Response(stream).arrayBuffer());
}

/**
 * Throws `TooLargeFile` when what rhwp would parse of the package as XML is
 * past the limit or its list is longer than rhwp looks up in good time
 * (`LOOKUP_LIMIT`), and `UnreadableFile` when its spine names a part twice, the
 * archive holds two parts of one name (rhwp reads the last, so the list read
 * here would not be its list), or the list is markup rhwp's reader stops at
 * or has a tag of more than 1,000 attributes (hpf-markup.ts).
 */
export async function checkHwpxPackage(archive: ArrayBuffer): Promise<void> {
  const bytes = new Uint8Array(archive);
  const entries = zipEntries(bytes);
  if (!entries) throw new UnreadableFile();
  if (new Set(entries.map((entry) => entry.name)).size !== entries.length) {
    throw new UnreadableFile();
  }
  const main = entries.find((entry) => entry.name === PACKAGE);
  // The list is XML to rhwp, whatever it starts with: held to the limit before it is read.
  if (main && main.size > MARKUP_LIMIT_BYTES) throw new TooLargeFile();
  const list = main ? listed(await partText(bytes, main)) : { items: [], spine: [] };
  const sections = sectionsOf(list);
  if (new Set(sections).size !== sections.length) throw new UnreadableFile();
  const masterPages = new Set(masterPagesOf(list));
  const parsed = new Set([...sections, ...masterPages]);
  let total = 0;
  for (const entry of entries) {
    if (entry.name.startsWith('BinData/') && !parsed.has(entry.name)) continue;
    const times = masterPages.has(entry.name) ? Math.max(1, sections.length) : 1;
    total += times * entry.size;
    if (total > MARKUP_LIMIT_BYTES) throw new TooLargeFile();
  }
}
