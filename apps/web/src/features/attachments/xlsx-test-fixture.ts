/**
 * Synthetic XLSX for viewer tests (unit and browser); never bundled.
 *
 * Hand-written SpreadsheetML in a ZIP produced by the tiny writer below —
 * independent of @office-kit/xlsx, the reader under test. Cells can be shared
 * strings, inline strings, numbers or formulas with a cached value; sheets can
 * carry merged ranges; a chartsheet slot has no cells. Optional parts add a
 * VBA project and an external workbook link whose formulas must only ever show
 * their cached values.
 */

export const XLSX_MIME = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
export const FIXTURE_EXTERNAL_WORKBOOK = "https://example.com/xlsx-external.xlsx";

export type FixtureCell =
  | { ref: string; shared: string }
  | { ref: string; inline: string }
  | { ref: string; number: number }
  | { ref: string; formula: string; cached: number | string };

export type FixtureSheet =
  { name: string; cells: FixtureCell[]; merges?: string[] } | { name: string; chart: true };

export type FixtureOptions = {
  /** Adds `xl/vbaProject.bin` (opaque bytes) with its workbook relationship. */
  vba?: boolean;
  /** Adds an external link part pointing at {@link FIXTURE_EXTERNAL_WORKBOOK}. */
  externalLink?: boolean;
  /** Raw-DEFLATE compress every part (default: stored). */
  deflate?: boolean;
};

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (const byte of bytes) c = CRC_TABLE[(c ^ byte) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

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

type ZipEntry = { name: string; data: Uint8Array; body: Uint8Array; method: 0 | 8 };

/** Minimal ZIP (APPNOTE 6.3): stored or raw-DEFLATE entries, UTF-8 names, no ZIP64. */
function writeZip(entries: ZipEntry[]): Uint8Array {
  const local: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  for (const entry of entries) {
    const name = new TextEncoder().encode(entry.name);
    const crc = crc32(entry.data);
    const common = (view: DataView, at: number) => {
      view.setUint16(at, 20, true);
      view.setUint16(at + 2, 0x0800, true);
      view.setUint16(at + 4, entry.method, true);
      view.setUint16(at + 6, 0, true);
      view.setUint16(at + 8, 0x5b21, true);
      view.setUint32(at + 10, crc, true);
      view.setUint32(at + 14, entry.body.byteLength, true);
      view.setUint32(at + 18, entry.data.byteLength, true);
      view.setUint16(at + 22, name.byteLength, true);
    };
    local.push(
      header(30, (view) => {
        view.setUint32(0, 0x04034b50, true);
        common(view, 4);
      }),
      name,
      entry.body,
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
    offset += 30 + name.byteLength + entry.body.byteLength;
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

async function deflateRaw(data: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([data as BlobPart])
    .stream()
    .pipeThrough(new CompressionStream("deflate-raw"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

function escapeXml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

const NS_MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_PKG_REL = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const XML_DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n';

function rowOf(ref: string): number {
  return Number(/\d+$/.exec(ref)![0]);
}

function worksheetXml(
  cells: FixtureCell[],
  merges: string[],
  sharedIndex: Map<string, number>,
): string {
  const rows = new Map<number, string[]>();
  for (const cell of cells) {
    let xml: string;
    if ("shared" in cell) {
      xml = `<c r="${cell.ref}" t="s"><v>${sharedIndex.get(cell.shared)}</v></c>`;
    } else if ("inline" in cell) {
      xml = `<c r="${cell.ref}" t="inlineStr"><is><t xml:space="preserve">${escapeXml(cell.inline)}</t></is></c>`;
    } else if ("number" in cell) {
      xml = `<c r="${cell.ref}"><v>${cell.number}</v></c>`;
    } else {
      const type = typeof cell.cached === "string" ? ' t="str"' : "";
      xml = `<c r="${cell.ref}"${type}><f>${escapeXml(cell.formula)}</f><v>${escapeXml(String(cell.cached))}</v></c>`;
    }
    const row = rowOf(cell.ref);
    rows.set(row, [...(rows.get(row) ?? []), xml]);
  }
  const sheetData = [...rows.entries()]
    .sort(([a], [b]) => a - b)
    .map(([row, xml]) => `<row r="${row}">${xml.join("")}</row>`)
    .join("");
  const mergeXml =
    merges.length === 0
      ? ""
      : `<mergeCells count="${merges.length}">${merges.map((ref) => `<mergeCell ref="${ref}"/>`).join("")}</mergeCells>`;
  return `${XML_DECL}<worksheet xmlns="${NS_MAIN}" xmlns:r="${NS_REL}"><sheetData>${sheetData}</sheetData>${mergeXml}</worksheet>`;
}

/** Builds the workbook package; parts are stored unless `deflate` is set. */
export async function buildFixtureXlsx(
  sheets: FixtureSheet[],
  options: FixtureOptions = {},
): Promise<Uint8Array> {
  const shared: string[] = [];
  const sharedIndex = new Map<string, number>();
  for (const sheet of sheets) {
    if ("chart" in sheet) continue;
    for (const cell of sheet.cells) {
      if ("shared" in cell && !sharedIndex.has(cell.shared)) {
        sharedIndex.set(cell.shared, shared.length);
        shared.push(cell.shared);
      }
    }
  }

  const parts: { name: string; xml: string | Uint8Array }[] = [];
  const overrides: string[] = [];
  const wbRels: string[] = [];
  const sheetEntries: string[] = [];
  sheets.forEach((sheet, index) => {
    const n = index + 1;
    const rId = `rId${n}`;
    if ("chart" in sheet) {
      parts.push({
        name: `xl/chartsheets/sheet${n}.xml`,
        xml: `${XML_DECL}<chartsheet xmlns="${NS_MAIN}" xmlns:r="${NS_REL}"><sheetViews><sheetView workbookViewId="0"/></sheetViews></chartsheet>`,
      });
      overrides.push(
        `<Override PartName="/xl/chartsheets/sheet${n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.chartsheet+xml"/>`,
      );
      wbRels.push(
        `<Relationship Id="${rId}" Type="${REL}/chartsheet" Target="chartsheets/sheet${n}.xml"/>`,
      );
    } else {
      parts.push({
        name: `xl/worksheets/sheet${n}.xml`,
        xml: worksheetXml(sheet.cells, sheet.merges ?? [], sharedIndex),
      });
      overrides.push(
        `<Override PartName="/xl/worksheets/sheet${n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>`,
      );
      wbRels.push(
        `<Relationship Id="${rId}" Type="${REL}/worksheet" Target="worksheets/sheet${n}.xml"/>`,
      );
    }
    sheetEntries.push(`<sheet name="${escapeXml(sheet.name)}" sheetId="${n}" r:id="${rId}"/>`);
  });

  let extraRel = sheets.length;
  if (shared.length > 0) {
    extraRel += 1;
    parts.push({
      name: "xl/sharedStrings.xml",
      xml: `${XML_DECL}<sst xmlns="${NS_MAIN}" count="${shared.length}" uniqueCount="${shared.length}">${shared
        .map((text) => `<si><t xml:space="preserve">${escapeXml(text)}</t></si>`)
        .join("")}</sst>`,
    });
    overrides.push(
      '<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>',
    );
    wbRels.push(
      `<Relationship Id="rId${extraRel}" Type="${REL}/sharedStrings" Target="sharedStrings.xml"/>`,
    );
  }
  let externalReferences = "";
  if (options.externalLink) {
    extraRel += 1;
    parts.push({
      name: "xl/externalLinks/externalLink1.xml",
      xml: `${XML_DECL}<externalLink xmlns="${NS_MAIN}" xmlns:r="${NS_REL}"><externalBook r:id="rId1"><sheetNames><sheetName val="Remote"/></sheetNames></externalBook></externalLink>`,
    });
    parts.push({
      name: "xl/externalLinks/_rels/externalLink1.xml.rels",
      xml: `${XML_DECL}<Relationships xmlns="${NS_PKG_REL}"><Relationship Id="rId1" Type="${REL}/externalLinkPath" Target="${escapeXml(FIXTURE_EXTERNAL_WORKBOOK)}" TargetMode="External"/></Relationships>`,
    });
    overrides.push(
      '<Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/>',
    );
    wbRels.push(
      `<Relationship Id="rId${extraRel}" Type="${REL}/externalLink" Target="externalLinks/externalLink1.xml"/>`,
    );
    externalReferences = `<externalReferences><externalReference r:id="rId${extraRel}"/></externalReferences>`;
  }
  if (options.vba) {
    extraRel += 1;
    // Opaque bytes: a real VBA project is an OLE compound file; nothing may run it.
    parts.push({
      name: "xl/vbaProject.bin",
      xml: new TextEncoder().encode("FVOCI-FIXTURE-VBA-NOT-EXECUTABLE"),
    });
    overrides.push(
      '<Override PartName="/xl/vbaProject.bin" ContentType="application/vnd.ms-office.vbaProject"/>',
    );
    wbRels.push(
      `<Relationship Id="rId${extraRel}" Type="http://schemas.microsoft.com/office/2006/relationships/vbaProject" Target="vbaProject.bin"/>`,
    );
  }

  const all: { name: string; xml: string | Uint8Array }[] = [
    {
      name: "[Content_Types].xml",
      xml: `${XML_DECL}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>${overrides.join("")}</Types>`,
    },
    {
      name: "_rels/.rels",
      xml: `${XML_DECL}<Relationships xmlns="${NS_PKG_REL}"><Relationship Id="rId1" Type="${REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>`,
    },
    {
      name: "xl/workbook.xml",
      xml: `${XML_DECL}<workbook xmlns="${NS_MAIN}" xmlns:r="${NS_REL}"><sheets>${sheetEntries.join("")}</sheets>${externalReferences}</workbook>`,
    },
    {
      name: "xl/_rels/workbook.xml.rels",
      xml: `${XML_DECL}<Relationships xmlns="${NS_PKG_REL}">${wbRels.join("")}</Relationships>`,
    },
    ...parts,
  ];
  const entries: ZipEntry[] = [];
  for (const part of all) {
    const data = typeof part.xml === "string" ? new TextEncoder().encode(part.xml) : part.xml;
    entries.push(
      options.deflate
        ? { name: part.name, data, body: await deflateRaw(data), method: 8 }
        : { name: part.name, data, body: data, method: 0 },
    );
  }
  return writeZip(entries);
}

/** Column letters for a 1-based column index (1 → A, 27 → AA). */
export function columnLetters(col: number): string {
  let out = "";
  for (let n = col; n > 0; n = Math.floor((n - 1) / 26)) {
    out = String.fromCharCode(65 + ((n - 1) % 26)) + out;
  }
  return out;
}

/** `rows`×`cols` inline-string grid whose cell text is `R<row>C<col>`. */
export function gridSheet(name: string, rows: number, cols: number): FixtureSheet {
  const cells: FixtureCell[] = [];
  for (let r = 1; r <= rows; r += 1) {
    for (let c = 1; c <= cols; c += 1) {
      cells.push({ ref: `${columnLetters(c)}${r}`, inline: `R${r}C${c}` });
    }
  }
  return { name, cells };
}
