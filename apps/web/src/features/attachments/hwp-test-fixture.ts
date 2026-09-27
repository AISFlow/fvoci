import { crc32, deflateRawSync, inflateRawSync } from "node:zlib";

/**
 * Test-only HWPX builder (Node). It rewrites the body of an existing Hancom
 * HWPX package — the user-authored `compat/fixtures/sample.hwpx` — into one
 * paragraph per page, each after the first starting with a page break, and
 * keeps the package's header, styles and page setup. The result is a real
 * OWPML container that no HWP library produced.
 */

type Entry = { name: string; data: Uint8Array };

function u16(view: DataView, at: number): number {
  return view.getUint16(at, true);
}

function u32(view: DataView, at: number): number {
  return view.getUint32(at, true);
}

/** Reads a ZIP through its central directory (stored and deflated entries only). */
export function readZip(bytes: Uint8Array): Entry[] {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let eocd = bytes.length - 22;
  while (eocd >= 0 && u32(view, eocd) !== 0x06054b50) eocd -= 1;
  if (eocd < 0) throw new Error("zip: no end of central directory");
  const count = u16(view, eocd + 10);
  let at = u32(view, eocd + 16);
  const decoder = new TextDecoder();
  const entries: Entry[] = [];
  for (let i = 0; i < count; i += 1) {
    if (u32(view, at) !== 0x02014b50) throw new Error("zip: bad central header");
    const method = u16(view, at + 10);
    const size = u32(view, at + 20);
    const nameLen = u16(view, at + 28);
    const extraLen = u16(view, at + 30);
    const commentLen = u16(view, at + 32);
    const local = u32(view, at + 42);
    const name = decoder.decode(bytes.subarray(at + 46, at + 46 + nameLen));
    const dataAt = local + 30 + u16(view, local + 26) + u16(view, local + 28);
    const raw = bytes.subarray(dataAt, dataAt + size);
    if (method !== 0 && method !== 8) throw new Error(`zip: method ${method}`);
    entries.push({ name, data: method === 0 ? raw.slice() : new Uint8Array(inflateRawSync(raw)) });
    at += 46 + nameLen + extraLen + commentLen;
  }
  return entries;
}

/** Writes a ZIP; `mimetype` stays first and stored, as OCF packages require. */
export function writeZip(entries: readonly Entry[]): Uint8Array {
  const encoder = new TextEncoder();
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;
  for (const entry of entries) {
    const name = encoder.encode(entry.name);
    const stored = entry.name === "mimetype";
    const body = stored ? entry.data : new Uint8Array(deflateRawSync(entry.data));
    const crc = crc32(entry.data) >>> 0;
    const local = new Uint8Array(30 + name.length);
    const lv = new DataView(local.buffer);
    lv.setUint32(0, 0x04034b50, true);
    lv.setUint16(4, 20, true);
    lv.setUint16(6, 0x0800, true);
    lv.setUint16(8, stored ? 0 : 8, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, body.length, true);
    lv.setUint32(22, entry.data.length, true);
    lv.setUint16(26, name.length, true);
    local.set(name, 30);
    const central = new Uint8Array(46 + name.length);
    const cv = new DataView(central.buffer);
    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(4, 20, true);
    cv.setUint16(6, 20, true);
    cv.setUint16(8, 0x0800, true);
    cv.setUint16(10, stored ? 0 : 8, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, body.length, true);
    cv.setUint32(24, entry.data.length, true);
    cv.setUint16(28, name.length, true);
    cv.setUint32(42, offset, true);
    central.set(name, 46);
    locals.push(local, body);
    centrals.push(central);
    offset += local.length + body.length;
  }
  const centralSize = centrals.reduce((n, c) => n + c.length, 0);
  const end = new Uint8Array(22);
  const ev = new DataView(end.buffer);
  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, entries.length, true);
  ev.setUint16(10, entries.length, true);
  ev.setUint32(12, centralSize, true);
  ev.setUint32(16, offset, true);
  const out = new Uint8Array(offset + centralSize + end.length);
  let at = 0;
  for (const part of [...locals, ...centrals, end]) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

function escapeXml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/**
 * Replaces the template body with `pages` (one paragraph each). The first
 * template paragraph carries the section/page setup, so its text run is
 * replaced in place; later pages are new paragraphs with `pageBreak="1"`.
 */
export function buildFixtureHwpx(template: Uint8Array, pages: readonly string[]): Uint8Array {
  if (pages.length === 0) throw new Error("at least one page");
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const entries = readZip(template)
    .filter((entry) => !entry.name.startsWith("Preview/"))
    .map((entry) => {
      if (entry.name === "Contents/content.hpf") {
        const hpf = decoder
          .decode(entry.data)
          .replace(/<opf:item [^>]*href="Preview\/[^"]*"[^>]*\/>/g, "");
        return { name: entry.name, data: encoder.encode(hpf) };
      }
      if (entry.name !== "Contents/section0.xml") return entry;
      const xml = decoder.decode(entry.data);
      const first = xml.indexOf("<hp:p ");
      const close = xml.indexOf("</hp:p>", first) + "</hp:p>".length;
      const texts = [...xml.slice(first, close).matchAll(/<hp:t>[^<]*<\/hp:t>|<hp:t\/>/g)];
      if (first < 0 || texts.length !== 1) throw new Error("unexpected template section");
      const head = xml
        .slice(first, close)
        .replace(texts[0]![0], `<hp:t>${escapeXml(pages[0]!)}</hp:t>`)
        .replace(/<hp:linesegarray>.*?<\/hp:linesegarray>/s, "");
      const rest = pages
        .slice(1)
        .map(
          (text, index) =>
            `<hp:p id="${index + 1}" paraPrIDRef="0" styleIDRef="0" pageBreak="1" columnBreak="0" merged="0">` +
            `<hp:run charPrIDRef="0"><hp:t>${escapeXml(text)}</hp:t></hp:run></hp:p>`,
        )
        .join("");
      const body = `${xml.slice(0, first)}${head}${rest}${xml.slice(close)}`;
      return { name: entry.name, data: encoder.encode(body) };
    });
  return writeZip(entries);
}

/**
 * Three pages of distinct Korean text, about 1000 characters each, so source
 * chunking of the joined page text (1500-char target, paragraph boundaries,
 * 150 overlap) gives one chunk per page: `?chunk=N` opens page N+1.
 */
export const FIXTURE_PAGES: readonly string[] = [
  `첫째 쪽 한글 문서 ${"가나다라마바사아자차 ".repeat(90)}`.trim(),
  `둘째 쪽 검색 대상 ${"하늘과 바람과 별과 시 ".repeat(78)}`.trim(),
  `셋째 쪽 마지막 문단 ${"동해 물과 백두산이 ".repeat(90)}`.trim(),
];
