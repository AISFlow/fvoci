/**
 * PowerPoint presentations for the tests of the file viewer, made here
 * rather than kept as binaries: a deck in Korean of three slides with a
 * title and body text, a shape, a picture, a table, a group, a chart, a link
 * to a web page and a jump to another slide, and the writer they are made
 * with, which the hostile decks (`hostile-pptx.ts`) use too. Every part is
 * ours, so the tests carry no third-party content.
 */
import { png, zip, type ZipMember } from './viewer-files.ts';

const P = 'http://schemas.openxmlformats.org/presentationml/2006/main';
const A = 'http://schemas.openxmlformats.org/drawingml/2006/main';
const R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const C = 'http://schemas.openxmlformats.org/drawingml/2006/chart';
const PACKAGE_RELS = 'http://schemas.openxmlformats.org/package/2006/relationships';
const TYPE = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>';
export const NAMESPACES = `xmlns:p="${P}" xmlns:a="${A}" xmlns:r="${R}"`;

/** A 16:9 slide, in EMU. */
export const SLIDE = { cx: 12_192_000, cy: 6_858_000 };

/** A relationship of a part: its id, type (the last segment of the type's URI) and target. */
export interface Link {
  id: string;
  type: string;
  target: string;
  external?: boolean;
}

export interface SlideSpec {
  /** The shapes of the slide's tree, as XML. */
  shapes: string;
  /** The slide's whole part, when it is not to be made of `shapes`. */
  xml?: string;
  links?: Link[];
  /** The relationship to the slide's layout, when it is not the plain one to the deck's layout. */
  layout?: Link;
  /** Hidden from a show (PowerPoint's "Hide Slide"): `show="0"`. */
  hidden?: boolean;
}

export interface DeckSpec {
  slides: SlideSpec[];
  /** Its parts kept as they are rather than deflated: a deck of many parts is unpacked one part at a time. */
  stored?: boolean;
  /** Shapes the layout and the master draw on every slide. */
  layoutShapes?: string;
  masterShapes?: string;
  /** More of the layout's and master's relationships (their pictures). */
  layoutLinks?: Link[];
  masterLinks?: Link[];
  /** Parts under ppt/ that are not XML, or are XML not otherwise made here: pictures, charts. */
  parts?: { name: string; data: Buffer | string }[];
  size?: { cx: number; cy: number };
  /** More of the theme, as XML after its elements (`<a:extLst>…`). */
  themeExtra?: string;
  /** The slide list, when it is to say other than one entry for each slide. */
  slideList?: string;
  /** More of the presentation part, as XML after its elements (`<p:extLst>…`). */
  presentationExtra?: string;
  /** The attributes of the slide size, when they are not `cx` and `cy` of `size`. */
  sizeAttributes?: string;
  /** More of the presentation's relationships. */
  presentationLinks?: Link[];
}

const box = (x: number, y: number, cx: number, cy: number): string =>
  `<a:xfrm><a:off x="${x}" y="${y}"/><a:ext cx="${cx}" cy="${cy}"/></a:xfrm>`;

/** A run of text in the deck's Korean font. */
export const run = (text: string, size = 2400, extra = '', inner = ''): string =>
  `<a:r><a:rPr lang="ko-KR" sz="${size}"${extra}><a:latin typeface="맑은 고딕"/><a:ea typeface="맑은 고딕"/>${inner}</a:rPr><a:t>${text}</a:t></a:r>`;

/** A text box of paragraphs (each a run's XML) at a place, in EMU. */
export function textBox(
  id: number,
  [x, y, cx, cy]: [number, number, number, number],
  paragraphs: string[],
  fill = '',
): string {
  const body = paragraphs.map((each) => `<a:p>${each}</a:p>`).join('');
  return `<p:sp><p:nvSpPr><p:cNvPr id="${id}" name="Text ${id}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>${box(x, y, cx, cy)}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>${fill}</p:spPr><p:txBody><a:bodyPr wrap="square"/><a:lstStyle/>${body}</p:txBody></p:sp>`;
}

/** A filled shape of a preset geometry. */
export function shape(
  id: number,
  geometry: string,
  place: [number, number, number, number],
  color: string,
): string {
  const [x, y, cx, cy] = place;
  return `<p:sp><p:nvSpPr><p:cNvPr id="${id}" name="Shape ${id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>${box(x, y, cx, cy)}<a:prstGeom prst="${geometry}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="${color}"/></a:solidFill></p:spPr></p:sp>`;
}

/** A picture of the relationship `embed`; `effects` go into its blip (`<a:lum …/>`). */
export function picture(
  id: number,
  embed: string,
  place: [number, number, number, number],
  effects = '',
): string {
  const [x, y, cx, cy] = place;
  return `<p:pic><p:nvPicPr><p:cNvPr id="${id}" name="Picture ${id}"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="${embed}">${effects}</a:blip><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>${box(x, y, cx, cy)}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>`;
}

/** A table of rows of cells' text. */
export function table(
  id: number,
  place: [number, number, number, number],
  rows: string[][],
): string {
  const [x, y, cx, cy] = place;
  const columns = rows[0]?.length ?? 1;
  const grid = `<a:tblGrid>${`<a:gridCol w="${Math.floor(cx / columns)}"/>`.repeat(columns)}</a:tblGrid>`;
  const line = (side: string): string =>
    `<a:${side} w="12700"><a:solidFill><a:srgbClr val="404040"/></a:solidFill></a:${side}>`;
  const cell = (text: string): string =>
    `<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p>${run(text, 1600)}</a:p></a:txBody><a:tcPr>${['lnL', 'lnR', 'lnT', 'lnB'].map(line).join('')}</a:tcPr></a:tc>`;
  const body = rows
    .map((cells) => `<a:tr h="${Math.floor(cy / rows.length)}">${cells.map(cell).join('')}</a:tr>`)
    .join('');
  return `<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="${id}" name="Table ${id}"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="${x}" y="${y}"/><a:ext cx="${cx}" cy="${cy}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1"/>${grid}${body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>`;
}

/** A group of shapes, in a child space as large as its own. */
export function group(
  id: number,
  place: [number, number, number, number],
  children: string,
): string {
  const [x, y, cx, cy] = place;
  return `<p:grpSp><p:nvGrpSpPr><p:cNvPr id="${id}" name="Group ${id}"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="${x}" y="${y}"/><a:ext cx="${cx}" cy="${cy}"/><a:chOff x="${x}" y="${y}"/><a:chExt cx="${cx}" cy="${cy}"/></a:xfrm></p:grpSpPr>${children}</p:grpSp>`;
}

/** A chart of the relationship `chart`. */
export function chartFrame(
  id: number,
  chart: string,
  place: [number, number, number, number],
): string {
  const [x, y, cx, cy] = place;
  return `<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="${id}" name="Chart ${id}"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="${x}" y="${y}"/><a:ext cx="${cx}" cy="${cy}"/></p:xfrm><a:graphic><a:graphicData uri="${C}"><c:chart xmlns:c="${C}" r:id="${chart}"/></a:graphicData></a:graphic></p:graphicFrame>`;
}

/** A bar chart of a few values. */
export function barChart(values: number[]): string {
  const points = values
    .map((value, index) => `<c:pt idx="${index}"><c:v>${value}</c:v></c:pt>`)
    .join('');
  return `${XML}<c:chartSpace xmlns:c="${C}" xmlns:a="${A}"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:ser><c:idx val="0"/><c:val><c:numRef><c:numCache><c:ptCount val="${values.length}"/>${points}</c:numCache></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>`;
}

function relationships(links: readonly Link[]): string {
  const each = links.map(
    ({ id, type, target, external }) =>
      `<Relationship Id="${id}" Type="${TYPE}/${type}" Target="${target}"${external ? ' TargetMode="External"' : ''}/>`,
  );
  return `${XML}<Relationships xmlns="${PACKAGE_RELS}">${each.join('')}</Relationships>`;
}

const tree = (shapes: string): string =>
  `<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>${shapes}</p:spTree></p:cSld>`;

const COLOR_MAP =
  'bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"';

function theme(extra = ''): string {
  const colors = [
    ['dk1', '000000'],
    ['lt1', 'FFFFFF'],
    ['dk2', '1F2937'],
    ['lt2', 'E5E7EB'],
    ['accent1', '2E7DD2'],
    ['accent2', 'E67E22'],
    ['accent3', '27AE60'],
    ['accent4', '8E44AD'],
    ['accent5', 'C0392B'],
    ['accent6', '16A085'],
    ['hlink', '2563EB'],
    ['folHlink', '7C3AED'],
  ]
    .map(([name, value]) => `<a:${name}><a:srgbClr val="${value}"/></a:${name}>`)
    .join('');
  const font = '<a:latin typeface="맑은 고딕"/><a:ea typeface="맑은 고딕"/><a:cs typeface=""/>';
  const fill = '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>';
  const line = '<a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>';
  return `${XML}<a:theme xmlns:a="${A}" name="fvoci"><a:themeElements><a:clrScheme name="fvoci">${colors}</a:clrScheme><a:fontScheme name="fvoci"><a:majorFont>${font}</a:majorFont><a:minorFont>${font}</a:minorFont></a:fontScheme><a:fmtScheme name="fvoci"><a:fillStyleLst>${fill.repeat(3)}</a:fillStyleLst><a:lnStyleLst>${line.repeat(3)}</a:lnStyleLst><a:effectStyleLst>${'<a:effectStyle><a:effectLst/></a:effectStyle>'.repeat(3)}</a:effectStyleLst><a:bgFillStyleLst>${fill.repeat(3)}</a:bgFillStyleLst></a:fmtScheme></a:themeElements>${extra}</a:theme>`;
}

/** A presentation of the slides, one layout, one master and one theme. */
export function deck(spec: DeckSpec): Buffer {
  const { slides, size = SLIDE } = spec;
  const xml = (name: string, text: string): ZipMember => ({
    name,
    data: Buffer.from(text, 'utf8'),
  });
  const slideList =
    spec.slideList ??
    slides.map((_, index) => `<p:sldId id="${256 + index}" r:id="rIdSlide${index + 1}"/>`).join('');
  const overrides = slides
    .map(
      (_, index) =>
        `<Override PartName="/ppt/slides/slide${index + 1}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>`,
    )
    .join('');
  const types = `${XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>${overrides}</Types>`;
  const presentation = `${XML}<p:presentation ${NAMESPACES}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdMaster"/></p:sldMasterIdLst><p:sldIdLst>${slideList}</p:sldIdLst><p:sldSz ${spec.sizeAttributes ?? `cx="${size.cx}" cy="${size.cy}"`}/><p:notesSz cx="6858000" cy="9144000"/>${spec.presentationExtra ?? ''}</p:presentation>`;
  const presentationLinks: Link[] = [
    { id: 'rIdMaster', type: 'slideMaster', target: 'slideMasters/slideMaster1.xml' },
    { id: 'rIdTheme', type: 'theme', target: 'theme/theme1.xml' },
    ...slides.map((_, index) => ({
      id: `rIdSlide${index + 1}`,
      type: 'slide',
      target: `slides/slide${index + 1}.xml`,
    })),
    ...(spec.presentationLinks ?? []),
  ];
  const master = `${XML}<p:sldMaster ${NAMESPACES}>${tree(spec.masterShapes ?? '')}<p:clrMap ${COLOR_MAP}/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rIdLayout"/></p:sldLayoutIdLst></p:sldMaster>`;
  const layout = `${XML}<p:sldLayout ${NAMESPACES} type="blank">${tree(spec.layoutShapes ?? '')}<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>`;
  const members: ZipMember[] = [
    xml('[Content_Types].xml', types),
    xml(
      '_rels/.rels',
      `${XML}<Relationships xmlns="${PACKAGE_RELS}"><Relationship Id="rId1" Type="${TYPE}/officeDocument" Target="ppt/presentation.xml"/></Relationships>`,
    ),
    xml('ppt/presentation.xml', presentation),
    xml('ppt/_rels/presentation.xml.rels', relationships(presentationLinks)),
    xml('ppt/theme/theme1.xml', theme(spec.themeExtra)),
    xml('ppt/slideMasters/slideMaster1.xml', master),
    xml(
      'ppt/slideMasters/_rels/slideMaster1.xml.rels',
      relationships([
        { id: 'rIdLayout', type: 'slideLayout', target: '../slideLayouts/slideLayout1.xml' },
        { id: 'rIdTheme', type: 'theme', target: '../theme/theme1.xml' },
        ...(spec.masterLinks ?? []),
      ]),
    ),
    xml('ppt/slideLayouts/slideLayout1.xml', layout),
    xml(
      'ppt/slideLayouts/_rels/slideLayout1.xml.rels',
      relationships([
        { id: 'rIdMaster', type: 'slideMaster', target: '../slideMasters/slideMaster1.xml' },
        ...(spec.layoutLinks ?? []),
      ]),
    ),
  ];
  slides.forEach((slide, index) => {
    members.push(
      xml(
        `ppt/slides/slide${index + 1}.xml`,
        slide.xml ??
          `${XML}<p:sld ${NAMESPACES}${slide.hidden ? ' show="0"' : ''}>${tree(slide.shapes)}<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>`,
      ),
      xml(
        `ppt/slides/_rels/slide${index + 1}.xml.rels`,
        relationships([
          slide.layout ?? {
            id: 'rIdLayout',
            type: 'slideLayout',
            target: '../slideLayouts/slideLayout1.xml',
          },
          ...(slide.links ?? []),
        ]),
      ),
    );
  });
  for (const part of spec.parts ?? []) {
    members.push({
      name: `ppt/${part.name}`,
      data: typeof part.data === 'string' ? Buffer.from(part.data, 'utf8') : part.data,
    });
  }
  return zip(spec.stored ? members.map((member) => ({ ...member, stored: true })) : members);
}

/** The text the Korean deck carries, for the tests to look for. */
export const PPTX_TEXT = {
  title: '분기 보고: 2026년 3분기',
  body: '이 발표 자료는 한국어 견본입니다. 도형과 그림이 들어 있습니다.',
  link: '원본 보기',
  jump: '셋째 장으로',
  tableTitle: '일정표',
  cells: [
    ['항목', '담당', '기한'],
    ['설계 검토', '김민지', '11월 20일'],
    ['출시', '박서준', '12월 1일'],
  ],
  third: '셋째 장: 묶은 도형',
};

/** Where the deck's link points. */
export const PPTX_LINK = 'https://example.invalid/login';

/**
 * A deck in Korean of three slides: a title, body text, a shape, a
 * picture and a jump to the third slide; a table, a chart and a link to a
 * web page; a group of shapes.
 */
export function koreanPptx(): Buffer {
  const W = SLIDE.cx;
  const first: SlideSpec = {
    shapes: [
      textBox(
        2,
        [600_000, 400_000, W - 1_200_000, 1_000_000],
        [run(PPTX_TEXT.title, 4000, ' b="1"')],
      ),
      textBox(3, [600_000, 1_600_000, 6_000_000, 1_600_000], [run(PPTX_TEXT.body, 2000)]),
      shape(4, 'roundRect', [7_200_000, 1_600_000, 2_000_000, 1_400_000], 'E67E22'),
      picture(5, 'rIdPicture', [7_200_000, 3_400_000, 3_000_000, 1_500_000]),
      textBox(
        6,
        [600_000, 3_600_000, 4_000_000, 600_000],
        [
          run(
            PPTX_TEXT.jump,
            1800,
            '',
            '<a:hlinkClick r:id="rIdJump" action="ppaction://hlinksldjump"/>',
          ),
        ],
      ),
    ].join(''),
    links: [
      { id: 'rIdPicture', type: 'image', target: '../media/image1.png' },
      { id: 'rIdJump', type: 'slide', target: 'slide3.xml' },
    ],
  };
  const second: SlideSpec = {
    shapes: [
      textBox(
        2,
        [600_000, 300_000, W - 1_200_000, 800_000],
        [run(PPTX_TEXT.tableTitle, 3200, ' b="1"')],
      ),
      table(3, [600_000, 1_300_000, 6_000_000, 1_800_000], PPTX_TEXT.cells),
      chartFrame(4, 'rIdChart', [7_000_000, 1_300_000, 4_500_000, 3_000_000]),
      textBox(
        5,
        [600_000, 3_600_000, 4_000_000, 600_000],
        [run(PPTX_TEXT.link, 1800, '', '<a:hlinkClick r:id="rIdLink"/>')],
      ),
    ].join(''),
    links: [
      { id: 'rIdChart', type: 'chart', target: '../charts/chart1.xml' },
      { id: 'rIdLink', type: 'hyperlink', target: PPTX_LINK, external: true },
    ],
  };
  const third: SlideSpec = {
    shapes: [
      textBox(
        2,
        [600_000, 300_000, W - 1_200_000, 800_000],
        [run(PPTX_TEXT.third, 3200, ' b="1"')],
      ),
      group(
        3,
        [600_000, 1_400_000, 6_000_000, 3_000_000],
        [
          shape(4, 'ellipse', [600_000, 1_400_000, 2_800_000, 2_800_000], '2E7DD2'),
          shape(5, 'triangle', [3_800_000, 1_400_000, 2_800_000, 2_800_000], '27AE60'),
        ].join(''),
      ),
    ].join(''),
  };
  return deck({
    slides: [first, second, third],
    parts: [
      { name: 'media/image1.png', data: png(300, 150, [46, 125, 210]) },
      { name: 'charts/chart1.xml', data: barChart([3, 5, 2, 6]) },
    ],
  });
}

/** The text on slide `number` of `numberedPptx`. */
export const numberedText = (number: number): string => `${number}번 장`;

/**
 * A deck of `count` slides, each saying its number, of which those numbered
 * in `hidden` are hidden from a show.
 */
export function numberedPptx(count: number, hidden: readonly number[]): Buffer {
  return deck({
    slides: Array.from({ length: count }, (_, index) => ({
      shapes: textBox(
        2,
        [600_000, 600_000, SLIDE.cx - 1_200_000, 1_200_000],
        [run(numberedText(index + 1), 4000)],
      ),
      hidden: hidden.includes(index + 1),
    })),
  });
}
