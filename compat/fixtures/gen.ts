#!/usr/bin/env bun
/**
 * Generate format-valid PDF/DOCX/HWPX/HWP representatives from public
 * container specs only (PDF 1.4, ECMA-376 OPC, PKWARE APPNOTE ZIP, MS-CFB v3).
 *
 *   bun compat/fixtures/gen.ts --output-dir <empty-directory>
 *
 * Output is deterministic: every ZIP entry carries FIXED_ZIP_TIME, which is
 * the timestamp of the checked-in sample.docx, so sample.pdf and sample.docx
 * reproduce the checked-in files byte for byte. The checked-in sample.hwp and
 * sample.hwpx are owner-authored Hancom files (NOTICE.md); the synthetic ones
 * written here never replace them.
 */
import { existsSync, mkdirSync, readdirSync, realpathSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { crc32, deflateRawSync } from "node:zlib";

const enc = new TextEncoder();

function concat(parts: readonly Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

export function buildPdf(): Uint8Array {
  // Minimal PDF 1.4, uncompressed content stream, Helvetica.
  const stream = enc.encode("BT /F1 12 Tf 72 720 Td (compat probe) Tj ET\n");
  const obj = (n: number, payload: Uint8Array) =>
    concat([enc.encode(`${n} 0 obj\n`), payload, enc.encode("\nendobj\n")]);
  const objs = [
    obj(1, enc.encode("<< /Type /Catalog /Pages 2 0 R >>")),
    obj(2, enc.encode("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")),
    obj(
      3,
      enc.encode(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] " +
          "/Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
      ),
    ),
    obj(
      4,
      concat([
        enc.encode(`<< /Length ${stream.length} >>\nstream\n`),
        stream,
        enc.encode("endstream"),
      ]),
    ),
    obj(5, enc.encode("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")),
  ];
  // Binary comment bytes per PDF 1.4 §3.4.1 so transfer tools treat it as binary.
  const header = Uint8Array.of(...enc.encode("%PDF-1.4\n%"), 0xe2, 0xe3, 0xcf, 0xd3, 0x0a);
  const xref = ["xref\n0 6\n0000000000 65535 f \n"];
  let pos = header.length;
  for (const o of objs) {
    xref.push(`${String(pos).padStart(10, "0")} 00000 n \n`);
    pos += o.length;
  }
  const trailer = `trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${pos}\n%%EOF\n`;
  return concat([header, ...objs, enc.encode(xref.join("") + trailer)]);
}

/** DOS date/time (APPNOTE 4.4.6) shared by every ZIP entry: 2026-09-24 01:22:12. */
export const FIXED_ZIP_TIME = { year: 2026, month: 9, day: 24, hour: 1, minute: 22, second: 12 };

type ZipEntry = { name: string; data: string; deflate: boolean };

/**
 * ZIP writer with the field layout of the checked-in sample.docx:
 * version 2.0 made on Unix, no data descriptor, no extra fields, mode 0600.
 * Deflate is raw zlib; Bun's bundled zlib reproduces the checked-in streams
 * at level 9 (its level 6 differs from stock zlib's).
 */
export function buildZip(entries: readonly ZipEntry[]): Uint8Array {
  const t = FIXED_ZIP_TIME;
  const dosTime = (t.hour << 11) | (t.minute << 5) | (t.second >> 1);
  const dosDate = ((t.year - 1980) << 9) | (t.month << 5) | t.day;
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;
  for (const e of entries) {
    const name = enc.encode(e.name);
    const raw = enc.encode(e.data);
    const body = e.deflate ? new Uint8Array(deflateRawSync(raw, { level: 9 })) : raw;
    const crc = crc32(raw);
    const local = new Uint8Array(30 + name.length);
    const lv = new DataView(local.buffer);
    lv.setUint32(0, 0x04034b50, true);
    lv.setUint16(4, 20, true);
    lv.setUint16(6, 0, true);
    lv.setUint16(8, e.deflate ? 8 : 0, true);
    lv.setUint16(10, dosTime, true);
    lv.setUint16(12, dosDate, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, body.length, true);
    lv.setUint32(22, raw.length, true);
    lv.setUint16(26, name.length, true);
    local.set(name, 30);
    const central = new Uint8Array(46 + name.length);
    const cv = new DataView(central.buffer);
    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(4, (3 << 8) | 20, true);
    cv.setUint16(6, 20, true);
    central.set(local.subarray(6, 28), 8); // flags through name length
    cv.setUint32(38, 0o600 << 16, true);
    cv.setUint32(42, offset, true);
    central.set(name, 46);
    locals.push(local, body);
    centrals.push(central);
    offset += local.length + body.length;
  }
  const cd = concat(centrals);
  const end = new Uint8Array(22);
  const ev = new DataView(end.buffer);
  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, entries.length, true);
  ev.setUint16(10, entries.length, true);
  ev.setUint32(12, cd.length, true);
  ev.setUint32(16, offset, true);
  return concat([...locals, cd, end]);
}

export function buildDocx(): Uint8Array {
  const document = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>안녕 compat 🚀</w:t></w:r></w:p>
  </w:body>
</w:document>
`;
  const ctypes = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>
`;
  const rels = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>
`;
  return buildZip([
    { name: "[Content_Types].xml", data: ctypes, deflate: true },
    { name: "_rels/.rels", data: rels, deflate: true },
    { name: "word/document.xml", data: document, deflate: true },
  ]);
}

export function buildHwpx(): Uint8Array {
  const version = `<?xml version="1.0" encoding="UTF-8"?>
<ha:HWPApplicationSetting xmlns:ha="http://www.hancom.co.kr/hwpml/2011/application">
  <ha:version>5.0.0.0</ha:version>
</ha:HWPApplicationSetting>
`;
  const container = `<?xml version="1.0" encoding="UTF-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="Contents/content.hpf" media-type="application/hwpml-package+xml"/>
  </rootfiles>
</container>
`;
  const manifest = `<?xml version="1.0" encoding="UTF-8"?>
<odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">
  <odf:file-entry odf:full-path="/" odf:media-type="application/hwp+zip"/>
</odf:manifest>
`;
  const contentHpf = `<?xml version="1.0" encoding="UTF-8"?>
<opf:package xmlns:opf="http://www.hancom.co.kr/hwpml/2011/pkg">
  <opf:manifest>
    <opf:item id="header" href="header.xml"/>
    <opf:item id="section0" href="section0.xml"/>
  </opf:manifest>
</opf:package>
`;
  const header = `<?xml version="1.0" encoding="UTF-8"?>
<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>
`;
  const section = `<?xml version="1.0" encoding="UTF-8"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p>
    <hp:run>
      <hp:t>한글본문색인토큰 HWPX</hp:t>
    </hp:run>
  </hp:p>
</hs:sec>
`;
  // OCF: the stored `mimetype` entry must come first.
  return buildZip([
    { name: "mimetype", data: "application/hwp+zip", deflate: false },
    { name: "version.xml", data: version, deflate: true },
    { name: "META-INF/container.xml", data: container, deflate: true },
    { name: "META-INF/manifest.xml", data: manifest, deflate: true },
    { name: "Contents/content.hpf", data: contentHpf, deflate: true },
    { name: "Contents/header.xml", data: header, deflate: true },
    { name: "Contents/section0.xml", data: section, deflate: true },
  ]);
}

const ENDOFCHAIN = 0xfffffffe;
const FREESECT = 0xffffffff;
const FATSECT = 0xfffffffd;
const NOSTREAM = 0xffffffff;

function dirEntry(
  name: string,
  type: number,
  start: number,
  size: number,
  child = NOSTREAM,
): Uint8Array {
  const out = new Uint8Array(128);
  const v = new DataView(out.buffer);
  for (let i = 0; i < name.length; i++) v.setUint16(i * 2, name.charCodeAt(i), true);
  v.setUint16(64, name.length * 2 + 2, true);
  out[66] = type;
  out[67] = type === 0 ? 0 : 1; // used entries are black: a valid one-node red-black tree
  v.setUint32(68, NOSTREAM, true); // left sibling
  v.setUint32(72, NOSTREAM, true); // right sibling
  v.setUint32(76, child, true);
  v.setUint32(116, start, true);
  v.setBigUint64(120, BigInt(size), true);
  return out;
}

/**
 * OLE CFB v3 holding a FileHeader stream: public MS-CFB + HWP 5.0 file signature.
 * MS-CFB 2.6.3: a stream below the 4096-byte cutoff lives in the mini stream, so
 * FileHeader is mini sectors 0-3, chained by the mini FAT, inside the mini stream
 * container that the Root Entry points to.
 *
 * Sectors: 0 FAT, 1 directory, 2 mini FAT, 3 mini stream container.
 */
export function buildHwp(): Uint8Array {
  const sector = 512;
  const miniSector = 64;
  // HWP 5.0 FileHeader: 32-byte signature field, version and flags left zero.
  const payload = new Uint8Array(256);
  payload.set(enc.encode("HWP Document File"));
  const miniSectors = payload.length / miniSector;

  const table = (entries: number[]) => {
    const out = new Uint8Array(sector);
    const v = new DataView(out.buffer);
    for (let i = 0; i < sector / 4; i++) v.setUint32(i * 4, entries[i] ?? FREESECT, true);
    return out;
  };
  const fat = table([FATSECT, ENDOFCHAIN, ENDOFCHAIN, ENDOFCHAIN]);
  const miniFat = table(
    Array.from({ length: miniSectors }, (_, i) => (i + 1 < miniSectors ? i + 1 : ENDOFCHAIN)),
  );

  const dir = concat([
    dirEntry("Root Entry", 5, 3, payload.length, 1),
    dirEntry("FileHeader", 2, 0, payload.length),
    dirEntry("", 0, 0, 0),
    dirEntry("", 0, 0, 0),
  ]);
  const container = new Uint8Array(sector);
  container.set(payload);

  const head = new Uint8Array(sector);
  const hv = new DataView(head.buffer);
  head.set([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
  hv.setUint16(24, 0x003e, true); // minor version
  hv.setUint16(26, 0x0003, true); // major version 3
  hv.setUint16(28, 0xfffe, true); // byte order
  hv.setUint16(30, 9, true); // sector shift: 512
  hv.setUint16(32, 6, true); // mini sector shift: 64
  hv.setUint32(44, 1, true); // number of FAT sectors
  hv.setUint32(48, 1, true); // first directory sector
  hv.setUint32(56, 4096, true); // mini stream cutoff
  hv.setUint32(60, 2, true); // first mini FAT sector
  hv.setUint32(64, 1, true); // number of mini FAT sectors
  hv.setUint32(68, ENDOFCHAIN, true); // first DIFAT sector
  hv.setUint32(76, 0, true); // DIFAT[0] = sector 0
  for (let i = 1; i < 109; i++) hv.setUint32(76 + i * 4, FREESECT, true);
  return concat([head, fat, dir, miniFat, container]);
}

export const OUTPUTS: readonly (readonly [string, () => Uint8Array])[] = [
  ["sample.pdf", buildPdf],
  ["sample.docx", buildDocx],
  ["sample.hwpx", buildHwpx],
  ["sample.hwp", buildHwp],
];

const FIXTURE_DIR = dirname(realpathSync(import.meta.filename));
const USAGE = "usage: bun compat/fixtures/gen.ts --output-dir <empty-directory>";

/** Returns an error message, or null when `dir` may receive the specimens. */
export function refuseOutputDir(dir: string): string | null {
  if (!existsSync(dir)) return null;
  if (!statSync(dir).isDirectory()) return `output path is not a directory: ${dir}`;
  if (realpathSync(dir) === FIXTURE_DIR || readdirSync(dir).length > 0) {
    return "output directory must be empty and separate from checked-in fixtures";
  }
  return null;
}

function main(argv: string[]): number {
  let outputDir: string | undefined;
  try {
    ({ "output-dir": outputDir } = parseArgs({
      args: argv,
      options: { "output-dir": { type: "string" } },
      strict: true,
    }).values);
  } catch (err) {
    console.error(`${USAGE}\n${(err as Error).message}`);
    return 2;
  }
  if (!outputDir) {
    console.error(`${USAGE}\n--output-dir is required`);
    return 2;
  }
  const dir = resolve(outputDir);
  const refusal = refuseOutputDir(dir);
  if (refusal) {
    console.error(`${USAGE}\n${refusal}`);
    return 2;
  }
  mkdirSync(dir, { recursive: true });
  for (const [name, build] of OUTPUTS) writeFileSync(join(dir, name), build());
  for (const [name] of OUTPUTS) console.log(`${name} ${statSync(join(dir, name)).size} bytes`);
  return 0;
}

if (import.meta.main) process.exit(main(process.argv.slice(2)));
