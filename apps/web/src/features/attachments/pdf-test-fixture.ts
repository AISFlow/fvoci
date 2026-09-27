/**
 * Synthetic PDFs for viewer tests (unit and browser); never bundled.
 *
 * Each page carries one text line and a colour band marking the page:
 * red across the top or blue across the bottom. Latin text uses Helvetica
 * (standard 14, not embedded). Korean text uses a non-embedded Adobe-Korea1
 * CID font with the predefined `UniKS-UCS2-H` CMap, so rendering it needs
 * pdf.js's packed CMaps (`cMapUrl`) — no font binary is included.
 */
export const FIXTURE_PAGE_W = 400;
export const FIXTURE_PAGE_H = 300;

export type FixturePage = {
  text: string;
  script: "latin" | "korean";
  band: "top-red" | "bottom-blue";
};

function ucs2Hex(text: string): string {
  let hex = "";
  for (let i = 0; i < text.length; i += 1) {
    hex += text.charCodeAt(i).toString(16).padStart(4, "0").toUpperCase();
  }
  return `<${hex}>`;
}

function latinLiteral(text: string): string {
  if (!/^[\x20-\x7e]*$/.test(text)) throw new Error("latin fixture text must be printable ASCII");
  return `(${text.replace(/[\\()]/g, (c) => `\\${c}`)})`;
}

export function buildFixturePdf(pages: FixturePage[]): Uint8Array {
  const objects: string[] = [];
  const add = (body: string) => {
    objects.push(body);
    return objects.length;
  };
  const catalog = add("");
  const pagesId = add("");
  const latin = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  const cid = add(
    "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /HYGoThic-Medium " +
      "/CIDSystemInfo << /Registry (Adobe) /Ordering (Korea1) /Supplement 2 >> " +
      "/FontDescriptor << /Type /FontDescriptor /FontName /HYGoThic-Medium /Flags 4 " +
      "/FontBBox [0 -148 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 93 >> " +
      "/DW 1000 >>",
  );
  const korean = add(
    `<< /Type /Font /Subtype /Type0 /BaseFont /HYGoThic-Medium-UniKS-UCS2-H ` +
      `/Encoding /UniKS-UCS2-H /DescendantFonts [${cid} 0 R] >>`,
  );
  const kids: number[] = [];
  for (const page of pages) {
    const band =
      page.band === "top-red"
        ? `1 0 0 rg 20 220 ${FIXTURE_PAGE_W - 40} 60 re f`
        : `0 0 1 rg 20 20 ${FIXTURE_PAGE_W - 40} 60 re f`;
    const show =
      page.script === "latin"
        ? `/F1 36 Tf 40 130 Td ${latinLiteral(page.text)} Tj`
        : `/F2 36 Tf 40 130 Td ${ucs2Hex(page.text)} Tj`;
    const stream = `${band}\n0 0 0 rg BT ${show} ET\n`;
    const content = add(`<< /Length ${stream.length} >>\nstream\n${stream}endstream`);
    kids.push(
      add(
        `<< /Type /Page /Parent ${pagesId} 0 R /MediaBox [0 0 ${FIXTURE_PAGE_W} ${FIXTURE_PAGE_H}] ` +
          `/Resources << /Font << /F1 ${latin} 0 R /F2 ${korean} 0 R >> >> /Contents ${content} 0 R >>`,
      ),
    );
  }
  objects[catalog - 1] = `<< /Type /Catalog /Pages ${pagesId} 0 R >>`;
  objects[pagesId - 1] =
    `<< /Type /Pages /Kids [${kids.map((k) => `${k} 0 R`).join(" ")}] /Count ${kids.length} >>`;

  // Every byte is ASCII, so string length is the byte offset.
  let out = "%PDF-1.4\n";
  const offsets: number[] = [];
  objects.forEach((body, index) => {
    offsets.push(out.length);
    out += `${index + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) out += `${String(offset).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${objects.length + 1} /Root ${catalog} 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(out);
}
