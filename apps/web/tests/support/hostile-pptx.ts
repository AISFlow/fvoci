/**
 * Small PowerPoint decks that would make the frame do a great deal: each
 * passes exactly one of the PowerPoint worker's ceilings
 * (src/viewer-frame/pptx-ceilings.ts), and no other, so that the tests show
 * each ceiling catches its case; and the same decks in disguise, which name
 * what they draw in ways the renderer reads otherwise than plainly. Pictures
 * that are to be large are a PNG header alone: the worker reads the size a
 * picture decodes to from its header, as the browser does, and never
 * decodes it.
 */
import { crc32 } from 'node:zlib';

import { DECK_LIMITS, SLIDE_LIMITS } from '../../src/viewer-frame/pptx-ceilings.ts';

import { png } from './viewer-files.ts';
import {
  deck,
  group,
  NAMESPACES,
  picture,
  run,
  shape,
  SLIDE,
  table,
  textBox,
  type Link,
} from './viewer-pptx.ts';

const PLACE: [number, number, number, number] = [600_000, 600_000, 4_000_000, 2_000_000];

/** A PNG of its header alone: as large as it says, to whatever reads only the header. */
export function pngHeader(width: number, height: number): Buffer {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const body = Buffer.concat([Buffer.from('IHDR', 'latin1'), header]);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(13);
  const sum = Buffer.alloc(4);
  sum.writeUInt32BE(crc32(body));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    length,
    body,
    sum,
  ]);
}

const one = (
  shapes: string,
  links: Link[] = [],
  parts: { name: string; data: Buffer }[] = [],
): Buffer => deck({ slides: [{ shapes, links }], parts });

const image = (name: string): Link => ({
  id: 'rIdPicture',
  type: 'image',
  target: `../media/${name}`,
});

/** Elements nested `depth` deep, inside a shape's extension list (which the renderer passes over). */
const nested = (depth: number): string =>
  `<a:extLst><a:ext uri="{fvoci}">${'<a:x>'.repeat(depth)}${'</a:x>'.repeat(depth)}</a:ext></a:extLst>`;

/** Every hostile deck, by the ceiling it passes (the name `TooMuch` is thrown with). */
export const HOSTILE_PPTX: Record<string, () => Buffer> = {
  // Stored: unpacking two thousand deflated parts one by one, twice, is most of a second.
  slides: () =>
    deck({
      slides: Array.from({ length: DECK_LIMITS.slides + 1 }, () => ({ shapes: '' })),
      stored: true,
    }),
  elements: () =>
    one(
      textBox(
        2,
        PLACE,
        Array.from({ length: SLIDE_LIMITS.elements }, () => ''),
      ),
    ),
  depth: () =>
    one(
      shape(2, 'rect', PLACE, '2E7DD2').replace(
        '</p:spPr>',
        `${nested(SLIDE_LIMITS.depth)}</p:spPr>`,
      ),
    ),
  text: () => one(textBox(2, PLACE, [run('가'.repeat(SLIDE_LIMITS.text + 1))])),
  tabs: () => one(textBox(2, PLACE, [run('\t'.repeat(SLIDE_LIMITS.tabs + 1))])),
  shapes: () =>
    one(
      Array.from({ length: SLIDE_LIMITS.shapes + 1 }, (_, index) =>
        shape(index + 2, 'rect', [index * 1000, 0, 1000, 1000], '2E7DD2'),
      ).join(''),
    ),
  textBodies: () =>
    one(
      Array.from({ length: SLIDE_LIMITS.textBodies + 1 }, (_, index) =>
        textBox(index + 2, [index * 1000, 0, 1000, 1000], [run('가')]),
      ).join(''),
    ),
  work: () => {
    // Seven tenths of three ceilings: under each, past them together.
    const share = (limit: number): number => Math.ceil(limit * 0.7);
    const texts = Array.from({ length: share(SLIDE_LIMITS.textBodies) }, (_, index) =>
      textBox(index + 2, [index * 1000, 0, 1000, 1000], [run('가')]),
    );
    const shapes = Array.from({ length: share(SLIDE_LIMITS.shapes) - texts.length }, (_, index) =>
      shape(index + 1000, 'rect', [index * 1000, 0, 1000, 1000], '2E7DD2'),
    );
    const paragraphs = textBox(
      5000,
      PLACE,
      Array.from({ length: share(SLIDE_LIMITS.elements) }, () => ''),
    );
    return one([...texts, ...shapes, paragraphs].join(''));
  },
  groups: () => {
    let inside = shape(100, 'rect', PLACE, '2E7DD2');
    for (let level = 0; level <= SLIDE_LIMITS.groups; level += 1)
      inside = group(level + 2, PLACE, inside);
    return one(inside);
  },
  cells: () => {
    const columns = 50;
    const rows = Math.floor(SLIDE_LIMITS.cells / columns) + 1;
    return one(
      table(
        2,
        PLACE,
        Array.from({ length: rows }, () => Array.from({ length: columns }, () => '')),
      ),
    );
  },
  pictures: () =>
    one(
      Array.from({ length: SLIDE_LIMITS.pictures + 1 }, (_, index) =>
        picture(index + 2, 'rIdPicture', PLACE),
      ).join(''),
      [image('image1.png')],
      [{ name: 'media/image1.png', data: png(4, 4, [46, 125, 210]) }],
    ),
  pixels: () => {
    // Pictures each under the ceiling of one, together past the slide's.
    const side = Math.floor(Math.sqrt(DECK_LIMITS.picture));
    const count = Math.floor(SLIDE_LIMITS.pixels / (side * side)) + 1;
    return one(
      Array.from({ length: count }, (_, index) => picture(index + 2, 'rIdPicture', PLACE)).join(''),
      [image('image1.png')],
      [{ name: 'media/image1.png', data: pngHeader(side, side) }],
    );
  },
  picture: () => {
    const side = Math.ceil(Math.sqrt(DECK_LIMITS.picture)) + 1;
    // Not shown anywhere: a picture that large is refused wherever it is.
    return one('', [], [{ name: 'media/image1.png', data: pngHeader(side, side) }]);
  },
  recoloured: () => {
    const side = Math.ceil(Math.sqrt(SLIDE_LIMITS.recoloured)) + 1;
    return one(
      picture(2, 'rIdPicture', PLACE, '<a:lum bright="10000"/>'),
      [image('image1.png')],
      [{ name: 'media/image1.png', data: pngHeader(side, side) }],
    );
  },
  extent: () => one(shape(2, 'rect', [DECK_LIMITS.extent + 1, 0, 1000, 1000], '2E7DD2')),
  'slide size': () =>
    deck({ slides: [{ shapes: '' }], size: { cx: DECK_LIMITS['slide size'] + 1, cy: SLIDE.cy } }),
  bytes: () =>
    one(
      shape(2, 'rect', PLACE, '2E7DD2').replace(
        '</p:spPr>',
        `<a:extLst><a:ext uri="${'x'.repeat(SLIDE_LIMITS.bytes)}"/></a:extLst></p:spPr>`,
      ),
    ),
  'bytes before the first slide': () =>
    deck({
      slides: [{ shapes: '' }],
      themeExtra: `<a:extLst><a:ext uri="${'x'.repeat(DECK_LIMITS['bytes before the first slide'])}"/></a:extLst>`,
    }),
  'elements before the first slide': () =>
    deck({
      slides: [{ shapes: '' }],
      themeExtra: `<a:extLst><a:ext uri="{fvoci}">${'<a:x/>'.repeat(DECK_LIMITS['elements before the first slide'])}</a:ext></a:extLst>`,
    }),
  // The renderer would draw the drawing inside itself, without end: never drawn in a test.
  'SmartArt in SmartArt': () => smartArtPptx(1, 1, diagramFrame(2)),
};

/**
 * A slide and its layout each under the ceiling of a slide's elements, past
 * it together: the layout's shapes are drawn on the slide.
 */
export function layoutAndSlidePptx(layoutTarget = '../slideLayouts/slideLayout1.xml'): Buffer {
  const half = Math.floor(SLIDE_LIMITS.elements / 2) + 10;
  const paragraphs = (count: number): string[] => Array.from({ length: count }, () => '');
  return deck({
    slides: [
      {
        shapes: textBox(2, PLACE, paragraphs(half)),
        layout: { id: 'rIdLayout', type: 'slideLayout', target: layoutTarget },
      },
    ],
    layoutShapes: textBox(2, PLACE, paragraphs(half)),
  });
}

/** A SmartArt frame: a diagram's `graphicFrame`, its parts named by the slide's relationships. */
const diagramFrame = (
  id: number,
  uri = 'uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"',
): string =>
  `<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="${id}" name="Diagram ${id}"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="1000" cy="1000"/></p:xfrm><a:graphic><a:graphicData ${uri}><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rIdData" r:lo="rIdLayoutData" r:qs="rIdStyle" r:cs="rIdColors"/></a:graphicData></a:graphic></p:graphicFrame>`;

/** A SmartArt drawing of `boxes` text boxes, and `more` (XML) after them. */
const drawing = (boxes: number, more = ''): string =>
  `<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" ${NAMESPACES}><dsp:spTree>${Array.from(
    { length: boxes },
    (_, index) =>
      `<dsp:sp modelId="{${index}}"><dsp:txBody><a:bodyPr/><a:p><a:r><a:t>가</a:t></a:r></a:p></dsp:txBody></dsp:sp>`,
  ).join('')}${more}</dsp:spTree></dsp:drawing>`;

/**
 * `frames` SmartArt frames that draw one drawing of `boxes` text boxes (and
 * `more`, XML), each frame once; `uri` names the frames' diagram.
 */
export function smartArtPptx(frames: number, boxes: number, more = '', uri?: string): Buffer {
  return one(
    Array.from({ length: frames }, (_, index) => diagramFrame(index + 2, uri)).join(''),
    [{ id: 'rIdDrawing', type: 'diagramDrawing', target: '../diagrams/drawing1.xml' }],
    [{ name: 'diagrams/drawing1.xml', data: Buffer.from(drawing(boxes, more)) }],
  );
}

const tinyPng = (): Buffer => png(1, 1, [0, 0, 0]);

/**
 * A paragraph with a tab stop and `tabs` tabs as elements between its runs,
 * which the renderer lays out as tabs (tab stops in `a:tabLst` are not).
 */
export const tabbed = (tabs: number): string =>
  `<a:pPr><a:tabLst><a:tab pos="914400" algn="l"/></a:tabLst></a:pPr>${run('가')}${'<a:tab/>'.repeat(tabs)}${run('나')}`;

/**
 * The hostile decks in disguise: each names what it draws in a way the
 * renderer reads otherwise than plainly, and is to be stopped at the same
 * ceiling as its plain twin (or, `unreadable`, refused as no file it reads).
 */
export const DISGUISED_PPTX: Record<string, { ceiling: string; make: () => Buffer }> = {
  'a layout named with a query': {
    ceiling: 'elements',
    make: () => layoutAndSlidePptx('../slideLayouts/slideLayout1.xml?x#y'),
  },
  'a layout named with backslashes': {
    ceiling: 'elements',
    make: () => layoutAndSlidePptx('..\\slideLayouts\\slideLayout1.xml'),
  },
  'a layout named with an escape': {
    ceiling: 'elements',
    make: () => layoutAndSlidePptx('../slideLayouts/slide%4Cayout1.xml'),
  },
  'a layout named from the root': {
    ceiling: 'elements',
    make: () => layoutAndSlidePptx('/ppt/slideLayouts/./slideLayout1.xml'),
  },
  'pictures linked rather than embedded': {
    ceiling: 'pictures',
    make: () =>
      one(
        Array.from({ length: SLIDE_LIMITS.pictures + 1 }, (_, index) =>
          picture(index + 2, 'rIdPicture', PLACE).replace('r:embed=', 'r:link='),
        ).join(''),
        [image('image1.png')],
        [{ name: 'media/image1.png', data: tinyPng() }],
      ),
  },
  'large pictures behind small ones (svgBlip)': {
    ceiling: 'pixels',
    make: () => {
      const side = Math.floor(Math.sqrt(DECK_LIMITS.picture));
      const count = Math.floor(SLIDE_LIMITS.pixels / (side * side)) + 1;
      const svg = `<a:extLst><a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}"><asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main" r:embed="rIdBig"/></a:ext></a:extLst>`;
      return one(
        Array.from({ length: count }, (_, index) =>
          picture(index + 2, 'rIdPicture', PLACE, svg),
        ).join(''),
        [image('image1.png'), { id: 'rIdBig', type: 'image', target: '../media/image2.png' }],
        [
          { name: 'media/image1.png', data: tinyPng() },
          { name: 'media/image2.png', data: pngHeader(side, side) },
        ],
      );
    },
  },
  'a picture recoloured three ways': {
    ceiling: 'recoloured',
    make: () => {
      // Under the ceiling twice over, past it three times over: each effect counts.
      const side = Math.ceil(Math.sqrt(SLIDE_LIMITS.recoloured / 3)) + 1;
      return one(
        picture(
          2,
          'rIdPicture',
          PLACE,
          '<a:lum bright="10000"/><a:biLevel thresh="50000"/><a:duotone/>',
        ),
        [image('image1.png')],
        [{ name: 'media/image1.png', data: pngHeader(side, side) }],
      );
    },
  },
  'a table of cells without columns': {
    ceiling: 'cells',
    make: () => {
      const cells = '<a:tc><a:txBody><a:bodyPr/><a:p/></a:txBody></a:tc>'.repeat(
        SLIDE_LIMITS.cells + 1,
      );
      return one(
        table(2, PLACE, [['']])
          .replace(/<a:tblGrid>.*<\/a:tblGrid>/, '<a:tblGrid/>')
          .replace(/<a:tr [^>]*>.*<\/a:tr>/, `<a:tr h="100">${cells}</a:tr>`),
      );
    },
  },
  'a slide no side tall': {
    ceiling: 'slide size',
    make: () => deck({ slides: [{ shapes: '' }], size: { cx: SLIDE.cx, cy: 0 } }),
  },
  'a huge slide behind a plain one, under the root': {
    ceiling: 'slide size',
    make: () =>
      deck({
        slides: [{ shapes: '' }],
        size: { cx: DECK_LIMITS['slide size'] + 1, cy: SLIDE.cy },
        presentationExtra: `<p:extLst><p:ext uri="{fvoci}"><p:sldSz cx="${SLIDE.cx}" cy="${SLIDE.cy}"/></p:ext></p:extLst>`,
      }),
  },
  'one SmartArt drawing drawn by many frames': {
    // Each frame alone is far under every ceiling: the drawing's boxes, once for each frame.
    ceiling: 'textBodies',
    make: () => smartArtPptx(40, 4),
  },
  'a SmartArt named by an attribute with a prefix besides one without': {
    ceiling: 'textBodies',
    make: () =>
      smartArtPptx(
        40,
        4,
        '',
        'xmlns:q="urn:fvoci" q:uri="x" uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"',
      ),
  },
  'a size read without its prefix, after a small one with one': {
    ceiling: 'extent',
    make: () =>
      one(
        shape(2, 'rect', PLACE, '2E7DD2').replace(
          /<a:ext cx="\d+" cy="\d+"\/>/,
          `<a:ext xmlns:q="urn:fvoci" q:cx="1" cx="${DECK_LIMITS.extent + 1}" cy="1000"/>`,
        ),
      ),
  },
  'a slide size read without its prefix': {
    ceiling: 'slide size',
    make: () =>
      deck({
        slides: [{ shapes: '' }],
        sizeAttributes: `xmlns:q="urn:fvoci" q:cx="${SLIDE.cx}" cx="${DECK_LIMITS['slide size'] + 1}" cy="${SLIDE.cy}"`,
      }),
  },
  'a column as wide as no slide': {
    ceiling: 'extent',
    make: () =>
      one(
        table(2, PLACE, [['']]).replace(
          /<a:gridCol w="\d+"\/>/,
          `<a:gridCol w="${DECK_LIMITS.extent + 1}"/>`,
        ),
      ),
  },
  'a place that is no finite number': {
    ceiling: 'extent',
    make: () =>
      one(shape(2, 'rect', PLACE, '2E7DD2').replace(/<a:off x="\d+"/, '<a:off x="Infinity"')),
  },
  'tabs as elements': {
    ceiling: 'tabs',
    make: () => one(textBox(2, PLACE, [tabbed(SLIDE_LIMITS.tabs + 1)])),
  },
  'text scaled up by its autofit': {
    ceiling: 'extent',
    make: () =>
      one(
        textBox(2, PLACE, [run('가', 400_000)]).replace(
          '<a:bodyPr wrap="square"/>',
          '<a:bodyPr wrap="square"><a:normAutofit fontScale="100000000"/></a:bodyPr>',
        ),
      ),
  },
  'lines spaced by more points than any font': {
    ceiling: 'extent',
    make: () =>
      one(
        textBox(2, PLACE, [
          `<a:pPr><a:lnSpc><a:spcPts val="1000000000000"/></a:lnSpc></a:pPr>${run('가')}`,
        ]),
      ),
  },
  'lines spaced by more lines than a slide holds': {
    ceiling: 'extent',
    make: () =>
      one(
        textBox(2, PLACE, [
          `<a:pPr><a:spcBef><a:spcPct val="1000000000000"/></a:spcBef></a:pPr>${run('가')}`,
        ]),
      ),
  },
  'an arc of a radius wider than any slide': {
    ceiling: 'extent',
    make: () =>
      one(
        shape(2, 'rect', PLACE, '2E7DD2').replace(
          /<a:prstGeom prst="rect">.*?<\/a:prstGeom>/,
          `<a:custGeom><a:pathLst><a:path w="1" h="1"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:arcTo wR="${DECK_LIMITS.extent + 1}" hR="1" stAng="0" swAng="5400000"/></a:path></a:pathLst></a:custGeom>`,
        ),
      ),
  },
  'text boxes under a slide root named as a table cell': {
    ceiling: 'textBodies',
    make: () => {
      const boxes = Array.from({ length: SLIDE_LIMITS.textBodies + 1 }, (_, index) =>
        textBox(index + 2, PLACE, ['']),
      ).join('');
      return deck({
        slides: [
          {
            shapes: '',
            xml: `<p:tc ${NAMESPACES}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>${boxes}</p:spTree></p:cSld></p:tc>`,
          },
        ],
      });
    },
  },
  'a table whose rows and columns are more than its cells': {
    ceiling: 'cells',
    make: () => {
      const columns = 50;
      const rows = Math.floor(SLIDE_LIMITS.cells / columns) + 1;
      const grid = '<a:gridCol w="10000"/>'.repeat(columns);
      const row = '<a:tr h="100"><a:tc><a:txBody><a:bodyPr/><a:p/></a:txBody></a:tc></a:tr>';
      return one(
        table(2, PLACE, [['']])
          .replace(/<a:tblGrid>.*<\/a:tblGrid>/, `<a:tblGrid>${grid}</a:tblGrid>`)
          .replace(/<a:tr [^>]*>.*<\/a:tr>/, row.repeat(rows)),
      );
    },
  },
  'text boxes on the master, which every slide draws': {
    ceiling: 'textBodies',
    make: () =>
      deck({
        slides: [{ shapes: '' }],
        masterShapes: Array.from({ length: SLIDE_LIMITS.textBodies + 1 }, (_, index) =>
          textBox(index + 2, PLACE, ['']),
        ).join(''),
      }),
  },
  "large pictures on the layout, by the layout's relationships": {
    ceiling: 'pixels',
    make: () => {
      const side = Math.floor(Math.sqrt(DECK_LIMITS.picture));
      const count = Math.floor(SLIDE_LIMITS.pixels / (side * side)) + 1;
      return deck({
        slides: [{ shapes: '' }],
        layoutShapes: Array.from({ length: count }, (_, index) =>
          picture(index + 2, 'rIdPicture', PLACE),
        ).join(''),
        layoutLinks: [image('image1.png')],
        parts: [{ name: 'media/image1.png', data: pngHeader(side, side) }],
      });
    },
  },
  'a picture the renderer finds under the last "media" of its name': {
    ceiling: 'pixels',
    make: () => {
      const side = Math.floor(Math.sqrt(DECK_LIMITS.picture));
      const count = Math.floor(SLIDE_LIMITS.pixels / (side * side)) + 1;
      return one(
        Array.from({ length: count }, (_, index) => picture(index + 2, 'rIdPicture', PLACE)).join(
          '',
        ),
        [{ id: 'rIdPicture', type: 'image', target: '../media/small/media/image1.png' }],
        [
          { name: 'media/small/media/image1.png', data: tinyPng() },
          { name: 'media/image1.png', data: pngHeader(side, side) },
        ],
      );
    },
  },
  'more slides listed than there are': {
    ceiling: 'slides',
    make: () =>
      deck({
        slides: [{ shapes: '' }],
        slideList: Array.from(
          { length: DECK_LIMITS.slides + 1 },
          (_, index) => `<p:sldId id="${256 + index}" r:id="rIdNone${index}"/>`,
        ).join(''),
      }),
  },
  'a slide listed twice': {
    ceiling: 'unreadable',
    make: () =>
      deck({
        slides: [{ shapes: '' }],
        slideList: '<p:sldId id="256" r:id="rIdSlide1"/><p:sldId id="257" r:id="rIdSlide1"/>',
      }),
  },
  'a slide two relationships of the presentation lead to': {
    ceiling: 'unreadable',
    make: () =>
      deck({
        slides: [{ shapes: '' }],
        slideList: '',
        presentationLinks: [{ id: 'rIdAgain', type: 'slide', target: 'slides/slide1.xml' }],
      }),
  },
  'a relationship given twice': {
    ceiling: 'unreadable',
    make: () =>
      one(
        picture(2, 'rIdPicture', PLACE),
        [image('image1.png'), { id: 'rIdPicture', type: 'image', target: '../media/image2.png' }],
        [
          { name: 'media/image1.png', data: tinyPng() },
          { name: 'media/image2.png', data: tinyPng() },
        ],
      ),
  },
};

/** A deck whose slide declares a document type, with an entity: no XML the frame reads as the worker does. */
export function doctypePptx(): Buffer {
  const xml = `<?xml version="1.0"?><!DOCTYPE p:sld [<!ENTITY big "가나다">]><p:sld ${NAMESPACES}><p:cSld><p:spTree/></p:cSld></p:sld>`;
  return deck({ slides: [{ shapes: '', xml }] });
}

/** A deck whose slide is not well formed. */
export function malformedPptx(): Buffer {
  return deck({ slides: [{ shapes: '', xml: `<p:sld ${NAMESPACES}><p:cSld>` }] });
}

/**
 * A deck of `slides` slides of `paragraphs` empty paragraphs each: under
 * every ceiling, but megabytes of XML that take the worker seconds to read.
 */
export function slowPptx(slides: number, paragraphs: number): Buffer {
  const shapes = textBox(
    2,
    PLACE,
    Array.from({ length: paragraphs }, () => ''),
  );
  return deck({ slides: Array.from({ length: slides }, () => ({ shapes })) });
}

/**
 * A deck of `slides` slides of `boxes` text boxes each: each slide takes the
 * frame a while to draw. The slides are small (`size`), so that many are in
 * view at once.
 */
export function heavyPptx(
  slides: number,
  boxes: number,
  size = { cx: 3_048_000, cy: 1_371_600 },
): Buffer {
  const shapes = Array.from({ length: boxes }, (_, index) =>
    textBox(
      index + 2,
      [(index % 20) * 600_000, Math.floor(index / 20) * 400_000, 600_000, 400_000],
      [run(`상자 ${index}`, 900)],
    ),
  ).join('');
  return deck({ slides: Array.from({ length: slides }, () => ({ shapes })), size });
}
