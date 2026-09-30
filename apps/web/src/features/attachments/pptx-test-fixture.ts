/**
 * Synthetic PPTX for viewer tests (unit and browser); never bundled.
 *
 * Hand-written PresentationML in a ZIP from the DOCX fixture's tiny writer —
 * independent of fflate/@office-kit, the reader under test. Two 16:9 slides
 * (960×540 CSS px). Slide 1 carries a Korean title, a text box with Korean +
 * emoji text, a bold run, an external and a `javascript:` hyperlink, a
 * two-level bullet list, a 2×2 table whose first cell is filled red, a green
 * rectangle and an orange ellipse, an embedded 4×2 blue PNG shown at 96×48 px
 * and a linked (external) picture that must never load. Slide 2 carries one
 * text box, optionally followed by `slide2Paragraphs` filler paragraphs of
 * `FIXTURE_PPTX_FILLER` (for layout-time tests: the renderer's layout is
 * super-linear in a text box's paragraph count).
 */

import { solidPng, writeZip } from "./docx-test-fixture.ts";

export const PPTX_MIME =
  "application/vnd.openxmlformats-officedocument.presentationml.presentation";
/** Slide size: 12192000 × 6858000 EMU = 960 × 540 CSS px. */
export const FIXTURE_PPTX_SLIDE_W = 960;
export const FIXTURE_PPTX_SLIDE_H = 540;
export const FIXTURE_PPTX_EXTERNAL_LINK = "https://example.com/pptx-link";
export const FIXTURE_PPTX_EXTERNAL_IMAGE = "https://example.com/pptx-tracker.png";
/** Fill colours the browser test samples from the rendered slide. */
export const FIXTURE_PPTX_COLORS = {
  red: [255, 0, 0],
  green: [0, 176, 80],
  orange: [255, 192, 0],
  blue: [0, 0, 255],
} as const;

const P = "http://schemas.openxmlformats.org/presentationml/2006/main";
const A = "http://schemas.openxmlformats.org/drawingml/2006/main";
const R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL = "http://schemas.openxmlformats.org/package/2006/relationships";
const TABLE_URI = "http://schemas.openxmlformats.org/drawingml/2006/table";
const XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n';

/** 1 CSS px = 9525 EMU. */
const px = (value: number) => Math.round(value * 9525);

function xmlText(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

const xfrm = (x: number, y: number, w: number, h: number) =>
  `<a:xfrm><a:off x="${String(px(x))}" y="${String(px(y))}"/><a:ext cx="${String(px(w))}" cy="${String(px(h))}"/></a:xfrm>`;

const solid = (hex: string) => `<a:solidFill><a:srgbClr val="${hex}"/></a:solidFill>`;

const hexOf = ([r, g, b]: readonly number[]) =>
  [r, g, b]
    .map((c) => c.toString(16).padStart(2, "0"))
    .join("")
    .toUpperCase();

/** A text run; `size` is in hundredths of a point (2000 = 20 pt = 26.67 CSS px). */
const run = (text: string, { size = 2000, bold = false, inner = "" } = {}) =>
  `<a:r><a:rPr lang="ko-KR" sz="${String(size)}"${bold ? ' b="1"' : ""}>${inner}</a:rPr><a:t>${xmlText(text)}</a:t></a:r>`;

const textBox = (
  id: number,
  name: string,
  x: number,
  y: number,
  w: number,
  h: number,
  paragraphs: string,
) =>
  `<p:sp><p:nvSpPr><p:cNvPr id="${String(id)}" name="${name}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>${xfrm(x, y, w, h)}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="square" lIns="0" tIns="0" rIns="0" bIns="0"/><a:lstStyle/>${paragraphs}</p:txBody></p:sp>`;

const shape = (
  id: number,
  name: string,
  preset: string,
  x: number,
  y: number,
  w: number,
  h: number,
  fill: string,
) =>
  `<p:sp><p:nvSpPr><p:cNvPr id="${String(id)}" name="${name}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>${xfrm(x, y, w, h)}<a:prstGeom prst="${preset}"><a:avLst/></a:prstGeom>${solid(fill)}<a:ln><a:noFill/></a:ln></p:spPr></p:sp>`;

const picture = (
  id: number,
  name: string,
  blip: string,
  x: number,
  y: number,
  w: number,
  h: number,
  rot = 0,
) =>
  `<p:pic><p:nvPicPr><p:cNvPr id="${String(id)}" name="${name}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill>${blip}<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>${rot ? xfrm(x, y, w, h).replace("<a:xfrm>", `<a:xfrm rot="${String(rot * 60000)}">`) : xfrm(x, y, w, h)}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>`;

/** A group whose children are laid out in `child` coordinates and scaled onto `outer`. */
const group = (
  id: number,
  outer: [number, number, number, number],
  child: [number, number, number, number],
  shapes: string,
) =>
  `<p:grpSp><p:nvGrpSpPr><p:cNvPr id="${String(id)}" name="Group ${String(id)}"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="${String(px(outer[0]))}" y="${String(px(outer[1]))}"/><a:ext cx="${String(px(outer[2]))}" cy="${String(px(outer[3]))}"/><a:chOff x="${String(px(child[0]))}" y="${String(px(child[1]))}"/><a:chExt cx="${String(px(child[2]))}" cy="${String(px(child[3]))}"/></a:xfrm></p:grpSpPr>${shapes}</p:grpSp>`;

const bullet = (text: string, level: number) =>
  `<a:p><a:pPr lvl="${String(level)}" marL="${String(px(24 + level * 32))}" indent="${String(px(-18))}"><a:buFont typeface="Arial"/><a:buChar char="${level === 0 ? "•" : "–"}"/></a:pPr>${run(text)}</a:p>`;

const border = (side: string) =>
  `<a:${side} w="12700"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:${side}>`;

const cell = (text: string, fill?: string) =>
  `<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p>${run(text, { size: 1400 })}</a:p></a:txBody><a:tcPr>${["lnL", "lnR", "lnT", "lnB"].map(border).join("")}${fill ? solid(fill) : ""}</a:tcPr></a:tc>`;

export type PptxFixtureText = {
  title: string;
  body: string;
  bold: string;
  link: string;
  scriptLink: string;
  list: [string, string, string];
  table: [string, string, string, string];
  secondSlide: string;
};

export const DEFAULT_PPTX_TEXT: PptxFixtureText = {
  title: "FVOCI PPTX 슬라이드",
  body: "한글 본문과 이모지 🙂🚀 ",
  bold: "굵은 글씨",
  link: "외부 링크",
  scriptLink: "스크립트 링크",
  list: ["첫째 항목", "하위 항목", "둘째 항목"],
  table: ["빨간 칸", "오른쪽 위", "왼쪽 아래", "오른쪽 아래"],
  secondSlide: "SECOND SLIDE 두 번째 슬라이드",
};

const slideXml = (tree: string) =>
  `${XML}<p:sld xmlns:a="${A}" xmlns:r="${R}" xmlns:p="${P}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>${tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>`;

const rels = (items: string[]) =>
  `${XML}<Relationships xmlns="${REL}">${items.join("")}</Relationships>`;
const rel = (id: string, type: string, target: string, external = false) =>
  `<Relationship Id="${id}" Type="${R}/${type}" Target="${xmlText(target)}"${external ? ' TargetMode="External"' : ""}/>`;

const THEME = `${XML}<a:theme xmlns:a="${A}" name="FVOCI"><a:themeElements><a:clrScheme name="FVOCI"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F2937"/></a:dk2><a:lt2><a:srgbClr val="F3F4F6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="FVOCI"><a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="FVOCI"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>`;

const EMPTY_TREE = `<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld>`;

const MASTER = `${XML}<p:sldMaster xmlns:a="${A}" xmlns:r="${R}" xmlns:p="${P}"><p:cSld><p:bg><p:bgPr>${solid("FFFFFF")}<a:effectLst/></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>`;

const LAYOUT = `${XML}<p:sldLayout xmlns:a="${A}" xmlns:r="${R}" xmlns:p="${P}" type="blank" preserve="1">${EMPTY_TREE.replace("<p:cSld>", '<p:cSld name="Blank">')}<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>`;

/** One filler paragraph (≈ 94 bytes of slide XML). */
export const FIXTURE_PPTX_FILLER = `<a:p><a:r><a:rPr lang="ko-KR" sz="1000"/><a:t>가나다라마바사 filler text 0123456789</a:t></a:r></a:p>`;

/**
 * `slide2Fallbacks` adds pictures the renderer can only draw as placeholders
 * to slide 2: a linked picture in a group scaled ×2 (child box 24×24 at
 * 10,10 → 48×48 at 120,320 on the slide), one rotated 90° (48×48 at
 * 400,300), an embed whose media part is missing (48×48 at 600,300) and a
 * linked one of zero size (at 800,300; the renderer draws nothing for it).
 */
export function buildFixturePptx(
  text: PptxFixtureText = DEFAULT_PPTX_TEXT,
  {
    slide2Paragraphs = 0,
    slide2Fallbacks = false,
  }: { slide2Paragraphs?: number; slide2Fallbacks?: boolean } = {},
): Uint8Array {
  const enc = (s: string) => new TextEncoder().encode(s);
  const contentTypes = `${XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>`;
  const presentation = `${XML}<p:presentation xmlns:a="${A}" xmlns:r="${R}" xmlns:p="${P}"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst><p:sldSz cx="${String(px(FIXTURE_PPTX_SLIDE_W))}" cy="${String(px(FIXTURE_PPTX_SLIDE_H))}"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>`;

  const table = `<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="6" name="Table"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="${String(px(480))}" y="${String(px(260))}"/><a:ext cx="${String(px(320))}" cy="${String(px(80))}"/></p:xfrm><a:graphic><a:graphicData uri="${TABLE_URI}"><a:tbl><a:tblPr/><a:tblGrid><a:gridCol w="${String(px(160))}"/><a:gridCol w="${String(px(160))}"/></a:tblGrid><a:tr h="${String(px(40))}">${cell(text.table[0], hexOf(FIXTURE_PPTX_COLORS.red))}${cell(text.table[1])}</a:tr><a:tr h="${String(px(40))}">${cell(text.table[2])}${cell(text.table[3])}</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>`;

  const slide1 = slideXml(
    [
      textBox(
        2,
        "Title",
        40,
        24,
        880,
        60,
        `<a:p>${run(text.title, { size: 2800, bold: true })}</a:p>`,
      ),
      textBox(
        3,
        "Body",
        40,
        100,
        880,
        40,
        `<a:p>${run(text.body)}${run(text.bold, { bold: true })}${run(" · ")}${run(text.link, { inner: '<a:hlinkClick r:id="rIdLink"/>' })}${run(" · ")}${run(text.scriptLink, { inner: '<a:hlinkClick r:id="rIdScript"/>' })}</a:p>`,
      ),
      textBox(
        4,
        "List",
        40,
        160,
        400,
        120,
        [bullet(text.list[0], 0), bullet(text.list[1], 1), bullet(text.list[2], 0)].join(""),
      ),
      table,
      shape(7, "Green rectangle", "rect", 40, 380, 160, 80, hexOf(FIXTURE_PPTX_COLORS.green)),
      shape(8, "Orange ellipse", "ellipse", 240, 380, 160, 80, hexOf(FIXTURE_PPTX_COLORS.orange)),
      picture(9, "Blue picture", '<a:blip r:embed="rIdImage"/>', 480, 400, 96, 48),
      picture(10, "Linked picture", '<a:blip r:link="rIdLinkedImage"/>', 640, 400, 48, 48),
    ].join(""),
  );
  const linked = '<a:blip r:link="rIdLinkedImage"/>';
  const fallbacks = slide2Fallbacks
    ? [
        group(
          3,
          [100, 300, 96, 96],
          [0, 0, 48, 48],
          picture(4, "Grouped linked picture", linked, 10, 10, 24, 24),
        ),
        picture(5, "Rotated linked picture", linked, 400, 300, 48, 48, 90),
        picture(6, "Missing picture", '<a:blip r:embed="rIdMissing"/>', 600, 300, 48, 48),
        picture(7, "Empty linked picture", linked, 800, 300, 0, 0),
      ].join("")
    : "";
  const slide2 = slideXml(
    textBox(
      2,
      "Second",
      40,
      40,
      880,
      60,
      `<a:p>${run(text.secondSlide, { size: 3200 })}</a:p>${FIXTURE_PPTX_FILLER.repeat(slide2Paragraphs)}`,
    ) + fallbacks,
  );

  return writeZip([
    { name: "[Content_Types].xml", bytes: enc(contentTypes) },
    {
      name: "_rels/.rels",
      bytes: enc(rels([rel("rId1", "officeDocument", "ppt/presentation.xml")])),
    },
    { name: "ppt/presentation.xml", bytes: enc(presentation) },
    {
      name: "ppt/_rels/presentation.xml.rels",
      bytes: enc(
        rels([
          rel("rId1", "slideMaster", "slideMasters/slideMaster1.xml"),
          rel("rId2", "slide", "slides/slide1.xml"),
          rel("rId3", "slide", "slides/slide2.xml"),
          rel("rId4", "theme", "theme/theme1.xml"),
        ]),
      ),
    },
    { name: "ppt/theme/theme1.xml", bytes: enc(THEME) },
    { name: "ppt/slideMasters/slideMaster1.xml", bytes: enc(MASTER) },
    {
      name: "ppt/slideMasters/_rels/slideMaster1.xml.rels",
      bytes: enc(
        rels([
          rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
          rel("rId2", "theme", "../theme/theme1.xml"),
        ]),
      ),
    },
    { name: "ppt/slideLayouts/slideLayout1.xml", bytes: enc(LAYOUT) },
    {
      name: "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
      bytes: enc(rels([rel("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")])),
    },
    { name: "ppt/slides/slide1.xml", bytes: enc(slide1) },
    {
      name: "ppt/slides/_rels/slide1.xml.rels",
      bytes: enc(
        rels([
          rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
          rel("rIdImage", "image", "../media/blue.png"),
          rel("rIdLink", "hyperlink", FIXTURE_PPTX_EXTERNAL_LINK, true),
          rel("rIdScript", "hyperlink", "javascript:alert(1)", true),
          rel("rIdLinkedImage", "image", FIXTURE_PPTX_EXTERNAL_IMAGE, true),
        ]),
      ),
    },
    { name: "ppt/slides/slide2.xml", bytes: enc(slide2) },
    {
      name: "ppt/slides/_rels/slide2.xml.rels",
      bytes: enc(
        rels([
          rel("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
          ...(slide2Fallbacks
            ? [
                rel("rIdLinkedImage", "image", FIXTURE_PPTX_EXTERNAL_IMAGE, true),
                rel("rIdMissing", "image", "../media/missing.png"),
              ]
            : []),
        ]),
      ),
    },
    { name: "ppt/media/blue.png", bytes: solidPng(4, 2, [...FIXTURE_PPTX_COLORS.blue]) },
  ]);
}
