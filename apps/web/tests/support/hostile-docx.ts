/**
 * Small Word documents that make docx-preview do a great deal: the cases the
 * reviews of the viewer found (each a few kilobytes, or less). The viewer
 * must end each within its limits: drawn, or refused as too large, without
 * the app's page waiting on it.
 */
import { png, zip } from './viewer-files.ts';

const W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main';
const R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const V = 'urn:schemas-microsoft-com:vml';
const PACKAGE_RELS = 'http://schemas.openxmlformats.org/package/2006/relationships';
const TYPE = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>';
const NAMESPACES = `xmlns:w="${W}" xmlns:r="${R}" xmlns:v="${V}"`;

const p = (text: string): string => `<w:p><w:r><w:t>${text}</w:t></w:r></w:p>`;
/** A paragraph that starts a page, where Word last broke it. */
const page = `<w:p><w:r><w:lastRenderedPageBreak/></w:r><w:r><w:t>쪽</w:t></w:r></w:p>`;

interface Parts {
  /** The body's paragraphs and tables, without the section's properties. */
  body: string;
  /** What the section's properties hold: header and footer references. */
  section?: string;
  /** Other parts, by name under word/, with their relationship type. */
  parts?: { name: string; type: string; xml: string }[];
  /** More of the document's relationships, as XML. */
  links?: string;
  /** Files under word/ that are not XML: pictures. */
  media?: { name: string; data: Buffer }[];
}

/** A Word document of the parts; relationship ids are the parts' names. */
export function wordDocx({
  body,
  section = '',
  parts = [],
  links = '',
  media = [],
}: Parts): Buffer {
  const rels =
    parts
      .map(
        (part) =>
          `<Relationship Id="${part.name}" Type="${TYPE}/${part.type}" Target="${part.name}.xml"/>`,
      )
      .join('') + links;
  return zip([
    {
      name: '_rels/.rels',
      data: Buffer.from(
        `${XML}<Relationships xmlns="${PACKAGE_RELS}"><Relationship Id="rId1" Type="${TYPE}/officeDocument" Target="word/document.xml"/></Relationships>`,
      ),
    },
    {
      name: 'word/document.xml',
      data: Buffer.from(
        `${XML}<w:document ${NAMESPACES}><w:body>${body}<w:sectPr>${section}</w:sectPr></w:body></w:document>`,
      ),
    },
    {
      name: 'word/_rels/document.xml.rels',
      data: Buffer.from(`${XML}<Relationships xmlns="${PACKAGE_RELS}">${rels}</Relationships>`),
    },
    ...parts.map((part) => ({
      name: `word/${part.name}.xml`,
      data: Buffer.from(`${XML}${part.xml}`),
    })),
    ...media.map((file) => ({ name: `word/${file.name}`, data: file.data })),
  ]);
}

/** Runs set in superscript, each holding a text box whose paragraph holds the next: docx-preview draws each run's content twice. */
export function nestedSuperscriptDocx(depth: number): Buffer {
  let run = '<w:r><w:t>x</w:t></w:r>';
  for (let level = 0; level < depth; level += 1) {
    run = `<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:pict><v:shape><v:textbox><w:txbxContent><w:p>${run}</w:p></w:txbxContent></v:textbox></v:shape></w:pict></w:r>`;
  }
  return wordDocx({ body: `<w:p>${run}</w:p>` });
}

/** A header holding a table of many columns, on many pages: a `col` for each, on each page. */
export function wideTableHeaderDocx(columns: number, pages: number): Buffer {
  const grid = '<w:gridCol w:w="10"/>'.repeat(columns);
  const header = `<w:hdr ${NAMESPACES}><w:tbl><w:tblGrid>${grid}</w:tblGrid><w:tr><w:tc>${p('a')}</w:tc></w:tr></w:tbl></w:hdr>`;
  return wordDocx({
    body: p('본문') + page.repeat(pages - 1),
    section: '<w:headerReference w:type="default" r:id="header1"/>',
    parts: [{ name: 'header1', type: 'header', xml: header }],
  });
}

/** Footnote references, each to a footnote of endnote references, each to a long endnote. */
export function notesInNotesDocx(references: number, inner: number, lines: number): Buffer {
  const footnotes = `<w:footnotes ${NAMESPACES}><w:footnote w:id="1"><w:p>${'<w:r><w:endnoteReference w:id="1"/></w:r>'.repeat(inner)}</w:p></w:footnote></w:footnotes>`;
  const endnotes = `<w:endnotes ${NAMESPACES}><w:endnote w:id="1">${p('미주').repeat(lines)}</w:endnote></w:endnotes>`;
  return wordDocx({
    body: `<w:p>${'<w:r><w:footnoteReference w:id="1"/></w:r>'.repeat(references)}</w:p>`,
    parts: [
      { name: 'footnotes', type: 'footnotes', xml: footnotes },
      { name: 'endnotes', type: 'endnotes', xml: endnotes },
    ],
  });
}

/** One list of many levels, used by many numberings: a style rule for each level of each. */
export function manyListLevelsDocx(levels: number, numberings: number): Buffer {
  const lvl = Array.from(
    { length: levels },
    (_, at) => `<w:lvl w:ilvl="${at}"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>`,
  ).join('');
  const nums = Array.from(
    { length: numberings },
    (_, at) => `<w:num w:numId="${at + 1}"><w:abstractNumId w:val="0"/></w:num>`,
  ).join('');
  const numbering = `<w:numbering ${NAMESPACES}><w:abstractNum w:abstractNumId="0">${lvl}</w:abstractNum>${nums}</w:numbering>`;
  return wordDocx({
    body: p('목록'),
    parts: [{ name: 'numbering', type: 'numbering', xml: numbering }],
  });
}

/** A long footer that the section names as its header too, on many pages. */
export function footerAsHeaderDocx(lines: number, pages: number): Buffer {
  const footer = `<w:ftr ${NAMESPACES}>${p('바닥글').repeat(lines)}</w:ftr>`;
  return wordDocx({
    body: p('본문') + page.repeat(pages - 1),
    section:
      '<w:headerReference w:type="default" r:id="footer1"/><w:footerReference w:type="default" r:id="footer1"/>',
    parts: [{ name: 'footer1', type: 'footer', xml: footer }],
  });
}

/** Styles each based on the one before: a long chain. */
export function styleChainDocx(length: number): Buffer {
  const styles = Array.from(
    { length },
    (_, at) =>
      `<w:style w:type="paragraph" w:styleId="s${at}">${at > 0 ? `<w:basedOn w:val="s${at - 1}"/>` : ''}<w:pPr><w:pageBreakBefore/></w:pPr></w:style>`,
  ).join('');
  return wordDocx({
    body: `<w:p><w:pPr><w:pStyle w:val="s${length - 1}"/></w:pPr><w:r><w:t>끝</w:t></w:r></w:p>`,
    parts: [
      { name: 'styles', type: 'styles', xml: `<w:styles ${NAMESPACES}>${styles}</w:styles>` },
    ],
  });
}

/** Many links, each with its own relationship: docx-preview looks each up among all of them. */
export function manyLinksDocx(count: number): Buffer {
  const indices = Array.from({ length: count }, (_, at) => at);
  return wordDocx({
    body: `<w:p>${indices.map((at) => `<w:hyperlink r:id="link${at}"><w:r><w:t>링크</w:t></w:r></w:hyperlink>`).join('')}</w:p>`,
    links: indices
      .map(
        (at) =>
          `<Relationship Id="link${at}" Type="${TYPE}/hyperlink" Target="https://example.invalid/${at}" TargetMode="External"/>`,
      )
      .join(''),
  });
}

/** A picture shown `count` times: one part, one relationship. */
export function picturesDocx(count: number): Buffer {
  const drawing = `<w:p><w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="952500" cy="952500"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:blipFill><a:blip r:embed="picture"/></pic:blipFill><pic:spPr><a:xfrm><a:ext cx="952500" cy="952500"/></a:xfrm></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>`;
  return wordDocx({
    body: drawing.repeat(count),
    links: `<Relationship Id="picture" Type="${TYPE}/image" Target="media/picture.png"/>`,
    media: [{ name: 'media/picture.png', data: png(10, 10, [200, 30, 30]) }],
  });
}

/** A run in a language. */
export function languageDocx(language: string): Buffer {
  return wordDocx({
    body: `<w:p><w:r><w:rPr><w:lang w:val="${language}"/></w:rPr><w:t>hello</w:t></w:r></w:p>`,
  });
}

/** A numbering part's XML under a root of any name: `numberings` numberings of a `levels`-level list. */
export function numberingXml(root: string, levels: number, numberings: number): string {
  const lvl = '<w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl>'.repeat(levels);
  const nums = Array.from(
    { length: numberings },
    (_, at) => `<w:num w:numId="${at + 1}"><w:abstractNumId w:val="0"/></w:num>`,
  ).join('');
  return `<${root} ${NAMESPACES}><w:abstractNum w:abstractNumId="0">${lvl}</w:abstractNum>${nums}</${root}>`;
}

/** `count` styles linked to one style of `entries` run properties: a rule for each, in each. */
export function linkedStylesDocx(count: number, entries: number): Buffer {
  const big = `<w:style w:type="character" w:styleId="big">${'<w:rPr><w:b/></w:rPr>'.repeat(entries)}</w:style>`;
  const linked = Array.from(
    { length: count },
    (_, at) => `<w:style w:type="paragraph" w:styleId="l${at}"><w:link w:val="big"/></w:style>`,
  ).join('');
  return wordDocx({
    body: p('본문'),
    parts: [
      { name: 'styles', type: 'styles', xml: `<w:styles ${NAMESPACES}>${big}${linked}</w:styles>` },
    ],
  });
}

/** `count` styles based on one table style of every conditional part: each gets a copy of all. */
export function basedOnTableStylesDocx(count: number): Buffer {
  const kinds = [
    'wholeTable',
    'firstRow',
    'lastRow',
    'firstCol',
    'lastCol',
    'band1Vert',
    'band2Vert',
    'band1Horz',
    'band2Horz',
    'neCell',
    'nwCell',
    'seCell',
    'swCell',
  ];
  const parts = kinds
    .map(
      (kind) =>
        `<w:tblStylePr w:type="${kind}"><w:rPr><w:b/></w:rPr><w:pPr><w:jc w:val="center"/></w:pPr><w:tcPr><w:shd w:fill="FF0000"/></w:tcPr></w:tblStylePr>`,
    )
    .join('');
  const base = `<w:style w:type="table" w:styleId="base">${parts}</w:style>`;
  const based = Array.from(
    { length: count },
    (_, at) => `<w:style w:type="table" w:styleId="t${at}"><w:basedOn w:val="base"/></w:style>`,
  ).join('');
  return wordDocx({
    body: p('본문'),
    parts: [
      { name: 'styles', type: 'styles', xml: `<w:styles ${NAMESPACES}>${base}${based}</w:styles>` },
    ],
  });
}

/** `count` empty paragraphs. */
export function emptyParagraphsDocx(count: number): Buffer {
  return wordDocx({ body: '<w:p/>'.repeat(count) });
}

/** `count` proofing marks in one paragraph: XML nodes that draw nothing. */
export function proofMarksDocx(count: number): Buffer {
  return wordDocx({ body: `<w:p>${'<w:proofErr w:type="spellStart"/>'.repeat(count)}</w:p>` });
}

/**
 * A document of the things docx-preview draws in its own ways: a table with
 * merged and spanned cells, a numbered list, a link out and a link within, a
 * bookmark, a footnote, a second section, a tab and a symbol.
 */
export function variedDocx(): Buffer {
  const cell = (text: string, props = ''): string =>
    `<w:tc><w:tcPr>${props}</w:tcPr>${p(text)}</w:tc>`;
  const table = `<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr>${cell('가', '<w:vMerge w:val="restart"/>')}${cell('나', '<w:gridSpan w:val="2"/>')}</w:tr><w:tr>${cell('', '<w:vMerge/>')}${cell('다')}${cell('라')}</w:tr></w:tbl>`;
  const list = `<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>항목</w:t></w:r></w:p>`;
  const links = `<w:p><w:bookmarkStart w:id="0" w:name="here"/><w:hyperlink r:id="out"><w:r><w:t>밖</w:t></w:r></w:hyperlink><w:hyperlink w:anchor="here"><w:r><w:t>안</w:t></w:r></w:hyperlink><w:bookmarkEnd w:id="0"/></w:p>`;
  const note = `<w:p><w:r><w:t>각주</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>`;
  const extras = `<w:p><w:r><w:t>탭</w:t><w:tab/><w:sym w:font="Symbol" w:char="F0B7"/></w:r></w:p>`;
  const sectionBreak = `<w:p><w:pPr><w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr></w:pPr><w:r><w:t>첫 구역</w:t></w:r></w:p>`;
  const numbering = `<w:numbering ${NAMESPACES}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>`;
  const footnotes = `<w:footnotes ${NAMESPACES}><w:footnote w:id="1">${p('각주의 글')}</w:footnote></w:footnotes>`;
  return wordDocx({
    body: table + list + links + note + extras + sectionBreak + p('둘째 구역'),
    parts: [
      { name: 'numbering', type: 'numbering', xml: numbering },
      { name: 'footnotes', type: 'footnotes', xml: footnotes },
    ],
    links: `<Relationship Id="out" Type="${TYPE}/hyperlink" Target="https://example.invalid/" TargetMode="External"/>`,
  });
}
