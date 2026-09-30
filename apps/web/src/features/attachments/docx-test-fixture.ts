/**
 * Synthetic DOCX for viewer tests (unit and browser); never bundled.
 *
 * Hand-written WordprocessingML in a ZIP produced by the tiny writer below —
 * independent of JSZip/docx-preview, the reader under test. Page 1 carries a
 * heading, Korean + emoji text, a bold run, an external and a `javascript:`
 * hyperlink, a two-level numbered list, a 2×2 bordered table whose first cell
 * is filled red, an embedded 4×2 blue PNG shown at 96×48 px and a linked
 * (external) image that must never load. An explicit page break starts page 2.
 * Letter page (8.5×11 in = 816×1056 CSS px), 1 in margins.
 */

export const DOCX_MIME = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
export const FIXTURE_DOCX_PAGE_W = 816;
export const FIXTURE_EXTERNAL_LINK = "https://example.com/docx-link";
export const FIXTURE_EXTERNAL_IMAGE = "https://example.com/docx-tracker.png";

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

export function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (const byte of bytes) {
    const entry = CRC_TABLE[(c ^ byte) & 0xff];
    if (entry === undefined) throw new Error("missing CRC table entry");
    c = entry ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

export type ZipEntry =
  | { name: string; bytes: Uint8Array }
  /** Pre-deflated (raw DEFLATE) body with the CRC and size of the inflated data. */
  | { name: string; deflated: Uint8Array; crc: number; size: number };

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.byteLength, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.byteLength;
  }
  return out;
}

function header(size: number, fill: (view: DataView) => void): Uint8Array {
  const bytes = new Uint8Array(size);
  fill(new DataView(bytes.buffer));
  return bytes;
}

/** Minimal ZIP (APPNOTE 6.3): stored or pre-deflated entries, UTF-8 names, no ZIP64. */
export function writeZip(entries: ZipEntry[]): Uint8Array {
  const local: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  for (const entry of entries) {
    const name = new TextEncoder().encode(entry.name);
    const stored = "bytes" in entry;
    const body = stored ? entry.bytes : entry.deflated;
    const crc = stored ? crc32(entry.bytes) : entry.crc;
    const size = stored ? entry.bytes.byteLength : entry.size;
    const common = (view: DataView, at: number) => {
      view.setUint16(at, 20, true);
      view.setUint16(at + 2, 0x0800, true);
      view.setUint16(at + 4, stored ? 0 : 8, true);
      view.setUint16(at + 6, 0, true);
      view.setUint16(at + 8, 0x5b21, true);
      view.setUint32(at + 10, crc, true);
      view.setUint32(at + 14, body.byteLength, true);
      view.setUint32(at + 18, size, true);
      view.setUint16(at + 22, name.byteLength, true);
    };
    local.push(
      header(30, (view) => {
        view.setUint32(0, 0x04034b50, true);
        common(view, 4);
      }),
      name,
      body,
    );
    central.push(
      header(46, (view) => {
        view.setUint32(0, 0x02014b50, true);
        view.setUint16(4, 20, true);
        common(view, 6);
        view.setUint32(42, offset, true);
      }),
      name,
    );
    offset += 30 + name.byteLength + body.byteLength;
  }
  const directory = concat(central);
  const end = header(22, (view) => {
    view.setUint32(0, 0x06054b50, true);
    view.setUint16(8, entries.length, true);
    view.setUint16(10, entries.length, true);
    view.setUint32(12, directory.byteLength, true);
    view.setUint32(16, offset, true);
  });
  return concat([...local, directory, end]);
}

/** zlib stream of stored (uncompressed) DEFLATE blocks, for the PNG IDAT. */
function zlibStored(data: Uint8Array): Uint8Array {
  const out: number[] = [0x78, 0x01];
  for (let at = 0; at < data.length || at === 0; at += 0xffff) {
    const block = data.subarray(at, Math.min(data.length, at + 0xffff));
    const last = at + 0xffff >= data.length;
    out.push(
      last ? 1 : 0,
      block.length & 0xff,
      block.length >> 8,
      ~block.length & 0xff,
      (~block.length >> 8) & 0xff,
    );
    out.push(...block);
    if (data.length === 0) break;
  }
  let a = 1;
  let b = 0;
  for (const byte of data) {
    a = (a + byte) % 65521;
    b = (b + a) % 65521;
  }
  out.push((b >> 8) & 0xff, b & 0xff, (a >> 8) & 0xff, a & 0xff);
  return new Uint8Array(out);
}

function pngChunk(type: string, data: Uint8Array): Uint8Array {
  const typed = concat([new TextEncoder().encode(type), data]);
  return concat([
    header(4, (view) => {
      view.setUint32(0, data.byteLength);
    }),
    typed,
    header(4, (view) => {
      view.setUint32(0, crc32(typed));
    }),
  ]);
}

/** Solid RGB PNG. */
export function solidPng(
  width: number,
  height: number,
  [r, g, b]: [number, number, number],
): Uint8Array {
  const rows: number[] = [];
  for (let y = 0; y < height; y += 1) {
    rows.push(0);
    for (let x = 0; x < width; x += 1) rows.push(r, g, b);
  }
  return concat([
    new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk(
      "IHDR",
      header(13, (view) => {
        view.setUint32(0, width);
        view.setUint32(4, height);
        view.setUint8(8, 8);
        view.setUint8(9, 2);
      }),
    ),
    pngChunk("IDAT", zlibStored(new Uint8Array(rows))),
    pngChunk("IEND", new Uint8Array(0)),
  ]);
}

const W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL = "http://schemas.openxmlformats.org/package/2006/relationships";
const WP = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const A = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PIC = "http://schemas.openxmlformats.org/drawingml/2006/picture";

function xmlText(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

const run = (text: string, props = "") =>
  `<w:r>${props ? `<w:rPr>${props}</w:rPr>` : ""}<w:t xml:space="preserve">${xmlText(text)}</w:t></w:r>`;

const listItem = (text: string, level: number) =>
  `<w:p><w:pPr><w:numPr><w:ilvl w:val="${String(level)}"/><w:numId w:val="1"/></w:numPr></w:pPr>${run(text)}</w:p>`;

const picture = (id: number, blip: string, cx: number, cy: number) =>
  `<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="${String(cx)}" cy="${String(cy)}"/><wp:docPr id="${String(id)}" name="picture ${String(id)}"/><a:graphic><a:graphicData uri="${PIC}"><pic:pic><pic:nvPicPr><pic:cNvPr id="${String(id)}" name="picture ${String(id)}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill>${blip}<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="${String(cx)}" cy="${String(cy)}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>`;

const cell = (text: string, fill?: string) =>
  `<w:tc><w:tcPr><w:tcW w:w="2400" w:type="dxa"/>${fill ? `<w:shd w:val="clear" w:color="auto" w:fill="${fill}"/>` : ""}</w:tcPr><w:p>${run(text)}</w:p></w:tc>`;

const border = (side: string) =>
  `<w:${side} w:val="single" w:sz="8" w:space="0" w:color="000000"/>`;

export type DocxFixtureText = {
  heading: string;
  body: string;
  bold: string;
  link: string;
  scriptLink: string;
  list: [string, string, string];
  table: [string, string, string, string];
  secondPage: string;
};

export const DEFAULT_DOCX_TEXT: DocxFixtureText = {
  heading: "FVOCI DOCX 레이아웃",
  body: "한글 본문과 이모지 🙂🚀 ",
  bold: "굵은 글씨",
  link: "외부 링크",
  scriptLink: "스크립트 링크",
  list: ["첫째 항목", "하위 항목", "둘째 항목"],
  table: ["빨간 칸", "오른쪽 위", "왼쪽 아래", "오른쪽 아래"],
  secondPage: "SECOND PAGE 두 번째 쪽",
};

export function buildFixtureDocx(text: DocxFixtureText = DEFAULT_DOCX_TEXT): Uint8Array {
  const enc = (s: string) => new TextEncoder().encode(s);
  const contentTypes = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/></Types>`;
  const rootRels = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="${REL}"><Relationship Id="rId1" Type="${R}/officeDocument" Target="word/document.xml"/></Relationships>`;
  const docRels = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="${REL}"><Relationship Id="rIdStyles" Type="${R}/styles" Target="styles.xml"/><Relationship Id="rIdNumbering" Type="${R}/numbering" Target="numbering.xml"/><Relationship Id="rIdImage" Type="${R}/image" Target="media/blue.png"/><Relationship Id="rIdLink" Type="${R}/hyperlink" Target="${FIXTURE_EXTERNAL_LINK}" TargetMode="External"/><Relationship Id="rIdScript" Type="${R}/hyperlink" Target="javascript:alert(1)" TargetMode="External"/><Relationship Id="rIdLinkedImage" Type="${R}/image" Target="${FIXTURE_EXTERNAL_IMAGE}" TargetMode="External"/></Relationships>`;
  const styles = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="${W}"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:rPr><w:b/><w:sz w:val="40"/></w:rPr></w:style><w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style></w:styles>`;
  const numbering = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="${W}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2)"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>`;
  const document = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="${W}" xmlns:r="${R}" xmlns:wp="${WP}" xmlns:a="${A}" xmlns:pic="${PIC}"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>${run(text.heading)}</w:p>
<w:p>${run(text.body)}${run(text.bold, "<w:b/>")}${run(" · ")}<w:hyperlink r:id="rIdLink">${run(text.link, '<w:rStyle w:val="Hyperlink"/>')}</w:hyperlink>${run(" · ")}<w:hyperlink r:id="rIdScript">${run(text.scriptLink, '<w:rStyle w:val="Hyperlink"/>')}</w:hyperlink></w:p>
${listItem(text.list[0], 0)}${listItem(text.list[1], 1)}${listItem(text.list[2], 0)}
<w:tbl><w:tblPr><w:tblW w:w="4800" w:type="dxa"/><w:tblBorders>${["top", "left", "bottom", "right", "insideH", "insideV"].map(border).join("")}</w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w="2400"/><w:gridCol w:w="2400"/></w:tblGrid><w:tr>${cell(text.table[0], "FF0000")}${cell(text.table[1])}</w:tr><w:tr>${cell(text.table[2])}${cell(text.table[3])}</w:tr></w:tbl>
<w:p>${picture(1, '<a:blip r:embed="rIdImage"/>', 914400, 457200)}${picture(2, '<a:blip r:link="rIdLinkedImage"/>', 457200, 457200)}</w:p>
<w:p><w:r><w:br w:type="page"/></w:r></w:p>
<w:p>${run(text.secondPage)}</w:p>
<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>
</w:body></w:document>`;
  return writeZip([
    { name: "[Content_Types].xml", bytes: enc(contentTypes) },
    { name: "_rels/.rels", bytes: enc(rootRels) },
    { name: "word/document.xml", bytes: enc(document) },
    { name: "word/_rels/document.xml.rels", bytes: enc(docRels) },
    { name: "word/styles.xml", bytes: enc(styles) },
    { name: "word/numbering.xml", bytes: enc(numbering) },
    { name: "word/media/blue.png", bytes: solidPng(4, 2, [0, 0, 255]) },
  ]);
}
