import { describe, expect, it } from 'vitest';

import { examine } from '$lib/file-viewer/formats.ts';
import { LIMITS, measureCeilings } from '../../src/viewer-frame/pptx-ceilings.ts';
import { pixelsOf } from '../../src/viewer-frame/pptx-images.ts';
import {
  resolveTarget,
  sortParts,
  writeRelationships,
  type Sorted,
} from '../../src/viewer-frame/pptx-parts.ts';
import { readFromPptxWorker } from '../../src/viewer-frame/pptx-protocol.ts';
import { readPresentation } from '../../src/viewer-frame/pptx-read.ts';
import { unpack } from '../../src/viewer-frame/pptx-unpack.ts';
import { repack } from '../../src/viewer-frame/zip-guard.ts';

import {
  DISGUISED_PPTX,
  doctypePptx,
  HOSTILE_PPTX,
  layoutAndSlidePptx,
  malformedPptx,
  pngHeader,
  smartArtPptx,
} from '../support/hostile-pptx.ts';
import { png, zip } from '../support/viewer-files.ts';
import {
  deck,
  koreanPptx,
  NAMESPACES,
  picture,
  PPTX_LINK,
  PPTX_TEXT,
  textBox,
} from '../support/viewer-pptx.ts';

/** The raw bytes of a deck, as the frame hands them to the PowerPoint worker. */
const raw = (file: Buffer): ArrayBuffer => Uint8Array.from(file).buffer;

/** The archive after `zip-guard` checks and rewrites it (used where `unpack` is called directly). */
const repacked = (file: Buffer): Promise<ArrayBuffer> => repack(raw(file));

const sorted = async (file: Buffer): Promise<Sorted> =>
  sortParts(await unpack(await repacked(file)));

/** Every ceiling a deck passes. */
const overOf = async (file: Buffer): Promise<string[]> => measureCeilings(await sorted(file)).over;

describe('a presentation is told apart by its first bytes', () => {
  it('a ZIP archive with ppt/presentation.xml', () => {
    expect(examine(Uint8Array.from(koreanPptx()))).toEqual({ kind: 'format', format: 'pptx' });
  });

  it('an old PowerPoint file (a compound file) is not one the viewer draws', () => {
    const compound = Buffer.alloc(1024);
    compound.set([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    expect(examine(Uint8Array.from(compound))).toEqual({ kind: 'unknown' });
  });
});

describe('the PowerPoint worker reads a presentation into the parts the renderer takes', () => {
  it('the Korean sample: its slides, picture and text; no chart, no link outside', async () => {
    const answer = await readPresentation(raw(koreanPptx()));
    if (answer.kind !== 'read') throw new Error(JSON.stringify(answer));
    const { parts } = answer;
    expect([...parts.slides.keys()].sort()).toEqual([
      'ppt/slides/slide1.xml',
      'ppt/slides/slide2.xml',
      'ppt/slides/slide3.xml',
    ]);
    expect(parts.slides.get('ppt/slides/slide1.xml')).toContain(PPTX_TEXT.title);
    expect([...parts.media.keys()]).toEqual(['ppt/media/image1.png']);
    expect(parts.charts.size).toBe(0);
    // The jump to a slide stays; the link to a web page is gone.
    const second = parts.slideRels.get('ppt/slides/_rels/slide2.xml.rels') ?? '';
    expect(second).not.toContain(PPTX_LINK);
    expect(second).not.toContain('rIdLink');
    expect(parts.slideRels.get('ppt/slides/_rels/slide1.xml.rels')).toContain('rIdJump');
    for (const character of '분기 보고') expect(answer.characters).toContain(character);
    // The answer survives the reading the frame gives it.
    expect(readFromPptxWorker(structuredClone(answer))).toEqual(answer);
  });

  it('a relationship to outside the file is left out, whatever it names', async () => {
    const file = deck({
      slides: [
        {
          shapes: picture(2, 'rIdOutside', [0, 0, 1000, 1000]),
          links: [
            {
              id: 'rIdOutside',
              type: 'image',
              target: 'https://example.invalid/a.png',
              external: true,
            },
          ],
        },
      ],
    });
    const parts = await sorted(file);
    expect(parts.parts.slideRels.get('ppt/slides/_rels/slide1.xml.rels')).not.toContain(
      'example.invalid',
    );
  });

  it('pictures of kinds a browser draws without running anything are kept, others left out', async () => {
    const svg = Buffer.from(
      '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>'.padEnd(64),
    );
    const emf = Buffer.alloc(64, 1);
    const file = deck({
      slides: [{ shapes: '' }],
      parts: [
        { name: 'media/image1.png', data: png(3, 2, [0, 0, 0]) },
        { name: 'media/image2.svg', data: svg },
        { name: 'media/image3.emf', data: emf },
      ],
    });
    const parts = await sorted(file);
    expect([...parts.parts.media.keys()]).toEqual(['ppt/media/image1.png']);
    expect(parts.pixels.get('ppt/media/image1.png')).toEqual({ width: 3, height: 2 });
  });

  it('a picture is kept only under a name of a kind the renderer hands to the browser', async () => {
    // The renderer reads a name ending in .emf itself, whatever its bytes say they are.
    const header = pngHeader(4000, 4000);
    const file = deck({
      slides: [{ shapes: '' }],
      parts: [
        { name: 'media/image1.emf', data: header },
        { name: 'media/image2.EMF', data: header },
        { name: 'media/image3.tif', data: header },
        { name: 'media/image4.JPG', data: png(2, 2, [0, 0, 0]) },
      ],
    });
    expect([...(await sorted(file)).parts.media.keys()]).toEqual(['ppt/media/image4.JPG']);
  });

  it('the answer says the pixels of the pictures each slide shows', async () => {
    const answer = await readPresentation(raw(koreanPptx()));
    if (answer.kind !== 'read') throw new Error(answer.reason);
    expect([...answer.pixels.keys()].sort()).toEqual([...answer.parts.slides.keys()].sort());
    expect(answer.pixels.get('ppt/slides/slide1.xml')).toBe(300 * 150);
    expect(answer.hidden).toEqual([]);
  });

  it('the answer names the slides hidden from a show, `show` read as the renderer reads it', async () => {
    const slide = (show: string): { shapes: string; xml: string } => ({
      shapes: '',
      xml: `<p:sld ${NAMESPACES} ${show}><p:cSld><p:spTree/></p:cSld></p:sld>`,
    });
    const file = deck({
      slides: [
        { shapes: '', hidden: true },
        slide('show="1"'),
        slide('show=" False "'),
        slide('show="off"'),
        slide('show="no"'),
        slide('p:show="0"'),
        slide('show="1" p:show="0"'),
      ],
    });
    const answer = await readPresentation(raw(file));
    if (answer.kind !== 'read') throw new Error(answer.reason);
    expect(answer.hidden.sort()).toEqual(
      [1, 3, 4, 6].map((number) => `ppt/slides/slide${number}.xml`),
    );
  });
});

describe('each ceiling stops the deck that passes it, and only that one', () => {
  it('the Korean sample passes them all', async () => {
    expect(await overOf(koreanPptx())).toEqual([]);
  });

  for (const [ceiling, make] of Object.entries(HOSTILE_PPTX)) {
    it(ceiling, async () => {
      expect(await overOf(make())).toEqual([ceiling]);
    });
  }

  it('a deck past a ceiling is refused as too large', async () => {
    for (const ceiling of ['text', 'slide size'] as const) {
      const file = HOSTILE_PPTX[ceiling]?.() ?? Buffer.alloc(0);
      expect(await readPresentation(raw(file))).toEqual({
        kind: 'failed',
        reason: 'too-large',
      });
    }
  });

  it("a slide is counted with its layout's shapes, which it draws", async () => {
    expect(await overOf(layoutAndSlidePptx())).toEqual(['elements']);
  });

  it('each SmartArt frame is counted with the drawing it draws', async () => {
    expect(await overOf(smartArtPptx(1, 4))).toEqual([]);
    expect(await overOf(smartArtPptx(40, 4))).toEqual(['textBodies']);
  });

  it('what PowerPoint writes besides is not counted as more: sections, tab stops', async () => {
    // A section lists its slides again; the renderer reads the slide list under the root only.
    const sections = `<p:extLst><p:ext uri="{521415D9-36F7-43E2-AB2F-B90AF26B5E84}"><p14:sectionLst xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main"><p14:section name="One" id="{00000000-0000-0000-0000-000000000001}"><p14:sldIdLst>${'<p14:sldId id="256"/>'.repeat(
      LIMITS.slides + 1,
    )}</p14:sldIdLst></p14:section></p14:sectionLst></p:ext></p:extLst>`;
    expect(await overOf(deck({ slides: [{ shapes: '' }], presentationExtra: sections }))).toEqual(
      [],
    );
    // Tab stops, many of them on many paragraphs: no tab laid out.
    const stops = `<a:pPr><a:tabLst>${'<a:tab pos="914400" algn="l"/>'.repeat(16)}</a:tabLst></a:pPr>`;
    const paragraphs = Array.from({ length: 70 }, () => stops);
    expect(
      await overOf(deck({ slides: [{ shapes: textBox(2, [0, 0, 100, 100], paragraphs) }] })),
    ).toEqual([]);
  });

  it('every ceiling has a deck', () => {
    expect(Object.keys(HOSTILE_PPTX).sort()).toEqual(Object.keys(LIMITS).sort());
  });
});

describe('a deck in disguise is stopped as its plain twin is', () => {
  for (const [disguise, { ceiling, make }] of Object.entries(DISGUISED_PPTX)) {
    it(`${disguise}: ${ceiling}`, async () => {
      const file = make();
      const answer = await readPresentation(raw(file));
      if (ceiling === 'unreadable') {
        expect(answer).toEqual({ kind: 'failed', reason: 'unreadable' });
        return;
      }
      expect(measureCeilings(sortParts(await unpack(await repacked(file)))).over).toEqual([
        ceiling,
      ]);
      expect(answer).toEqual({ kind: 'failed', reason: 'too-large' });
    });
  }
});

describe('the relationships the frame gets are the ones the worker counted', () => {
  it('each leads to a part kept, by its absolute name, of a type the renderer draws by', async () => {
    const file = deck({
      slides: [
        {
          shapes: '',
          layout: {
            id: 'rIdLayout',
            type: 'slideLayout',
            target: '..\\slideLayouts\\slideLayout1.xml?x',
          },
          links: [
            { id: 'rIdNotes', type: 'notesSlide', target: '../notesSlides/notesSlide1.xml' },
            { id: 'rIdNowhere', type: 'image', target: '../media/missing.png' },
            { id: 'rIdPlural', type: 'slideLayouts', target: '../slideLayouts/slideLayout2.xml' },
          ],
        },
      ],
      parts: [{ name: 'slideLayouts/slideLayout2.xml', data: `<p:sldLayout ${NAMESPACES}/>` }],
    });
    const { relationships, parts } = await sorted(file);
    expect(relationships.get('ppt/slides/_rels/slide1.xml.rels')).toEqual([
      {
        id: 'rIdLayout',
        type: 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout',
        target: '/ppt/slideLayouts/slideLayout1.xml',
      },
    ]);
    expect(parts.slideRels.get('ppt/slides/_rels/slide1.xml.rels')).toContain(
      'Target="/ppt/slideLayouts/slideLayout1.xml"',
    );
  });

  it('an identifier longer than a name is counted by, or with a line break, is left out', async () => {
    const long = 'r'.repeat(300);
    const file = deck({
      slides: [
        {
          shapes: '',
          links: [
            { id: long, type: 'image', target: '../media/image1.png' },
            { id: 'a&#10;b', type: 'image', target: '../media/image1.png' },
            { id: 'rIdTab', type: 'image', target: '../media/image1.png&#9;' },
            { id: 'rIdPicture', type: 'image', target: '../media/image1.png' },
          ],
        },
      ],
      parts: [{ name: 'media/image1.png', data: png(2, 2, [0, 0, 0]) }],
    });
    const kept = (await sorted(file)).relationships.get('ppt/slides/_rels/slide1.xml.rels');
    expect(kept?.map((each) => each.id)).toEqual(['rIdLayout', 'rIdPicture']);
  });

  it('a line break is written as a reference, which a parser keeps as it is', async () => {
    expect(writeRelationships([{ id: 'a\nb', type: 't\tu', target: '/x\ry' }])).toContain(
      'Id="a&#10;b" Type="t&#9;u" Target="/x&#13;y"',
    );
  });

  it('a part whose name the renderer would read otherwise is left out', async () => {
    const file = deck({
      slides: [{ shapes: '' }],
      parts: [
        { name: 'slides/slide%32.xml', data: `<p:sld ${NAMESPACES}/>` },
        { name: 'media/image%31.png', data: png(2, 2, [0, 0, 0]) },
      ],
    });
    const { parts } = await sorted(file);
    expect([...parts.slides.keys()]).toEqual(['ppt/slides/slide1.xml']);
    expect(parts.media.size).toBe(0);
  });
});

describe('a part the frame would read otherwise than the worker stops the file', () => {
  it('a document type, with entities', async () => {
    expect(await readPresentation(raw(doctypePptx()))).toEqual({
      kind: 'failed',
      reason: 'unreadable',
    });
  });

  it('XML that is not well formed', async () => {
    expect(await readPresentation(raw(malformedPptx()))).toEqual({
      kind: 'failed',
      reason: 'unreadable',
    });
  });

  it('an archive without a presentation', async () => {
    const archive = zip([{ name: 'ppt/slides/slide1.xml', data: Buffer.from('<p:sld/>') }]);
    expect(await readPresentation(raw(archive))).toEqual({
      kind: 'failed',
      reason: 'unreadable',
    });
  });

  it('a raw archive checked in the worker itself: stored size lie, backslash name, and overlapping parts', async () => {
    const baseEntries = async (): Promise<
      { name: string; data: Buffer; stored?: boolean; said?: number }[]
    > =>
      [...(await unpack(raw(deck({ slides: [{ shapes: '' }] }))))].map(([name, bytes]) => ({
        name,
        data: Buffer.from(bytes),
        stored: true,
      }));
    const base = await baseEntries();

    // Stored size lie: `pptx-unpack` does not check `entry.size` on stored entries; only `repack` catches it.
    const sizeLie = zip([
      ...base,
      { name: 'ppt/extra.bin', data: Buffer.alloc(1024, 0x41), stored: true, said: 10 },
    ]);
    expect((await unpack(raw(sizeLie))).has('ppt/slides/slide1.xml')).toBe(true);
    expect(await readPresentation(raw(sizeLie))).toEqual({
      kind: 'failed',
      reason: 'unreadable',
    });

    // Backslash entry name alongside a valid deck: `sortParts` would ignore it; only `repack` rejects it.
    const backslash = zip([
      ...base,
      { name: 'ppt\\extra.txt', data: Buffer.from('x'), stored: true },
    ]);
    expect((await unpack(raw(backslash))).has('ppt/slides/slide1.xml')).toBe(true);
    expect(await readPresentation(raw(backslash))).toEqual({
      kind: 'failed',
      reason: 'unreadable',
    });

    // Overlapping stored parts alongside a valid deck: `pptx-unpack` reads both; only `repack` rejects overlap.
    const overlapping = zip([
      { name: 'ppt/a.bin', data: Buffer.from('same-bytes'), stored: true },
      { name: 'ppt/b.bin', data: Buffer.from('same-bytes'), stored: true },
      ...base,
    ]);
    const directory = overlapping.readUInt32LE(overlapping.length - 6);
    const second = directory + 46 + 'ppt/a.bin'.length;
    overlapping.writeUInt32LE(0, second + 42);
    expect((await unpack(raw(overlapping))).has('ppt/slides/slide1.xml')).toBe(true);
    expect(await readPresentation(raw(overlapping))).toEqual({
      kind: 'failed',
      reason: 'unreadable',
    });
  });
});

describe('the size a picture decodes to is read from its header', () => {
  it('PNG', () => {
    expect(pixelsOf(png(7, 5, [0, 0, 0]))).toEqual({ width: 7, height: 5 });
    expect(pixelsOf(pngHeader(70_000, 50_000))).toEqual({ width: 70_000, height: 50_000 });
  });

  it('JPEG: its largest frame', () => {
    const segment = (marker: number, body: number[]): number[] => [
      0xff,
      marker,
      (body.length + 2) >> 8,
      (body.length + 2) & 0xff,
      ...body,
    ];
    const frame = (width: number, height: number): number[] =>
      segment(0xc0, [8, height >> 8, height & 0xff, width >> 8, width & 0xff, 1, 1, 0x11, 0]);
    const bytes = Uint8Array.from([
      0xff,
      0xd8,
      ...segment(
        0xe0,
        Array.from({ length: 14 }, () => 0),
      ),
      ...frame(10, 20),
      ...frame(3000, 2000),
      0xff,
      0xda,
      0,
      2,
    ]);
    expect(pixelsOf(bytes)).toEqual({ width: 3000, height: 2000 });
  });

  it('GIF: its screen grown to its frames', () => {
    const bytes = new Uint8Array(64);
    bytes.set(new TextEncoder().encode('GIF89a'));
    const view = new DataView(bytes.buffer);
    view.setUint16(6, 10, true);
    view.setUint16(8, 10, true);
    // No colour table; then an image of 500 by 40 at (100, 0).
    bytes[13] = 0x2c;
    view.setUint16(14, 100, true);
    view.setUint16(16, 0, true);
    view.setUint16(18, 500, true);
    view.setUint16(20, 40, true);
    expect(pixelsOf(bytes)).toEqual({ width: 600, height: 40 });
  });

  it('nothing else', () => {
    expect(
      pixelsOf(Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>'.padEnd(64))),
    ).toBeUndefined();
    expect(pixelsOf(new Uint8Array(8))).toBeUndefined();
  });
});

describe('a relationship leads where the renderer looks (its `Kt`)', () => {
  it('relative to its part, up and down, or from the root', () => {
    expect(resolveTarget('ppt/slides', '../media/a.png')).toBe('ppt/media/a.png');
    expect(resolveTarget('ppt/slides', 'slide2.xml')).toBe('ppt/slides/slide2.xml');
    expect(resolveTarget('ppt/slides', '/ppt/media/b.png')).toBe('ppt/media/b.png');
  });

  it('without its query or fragment, its backslashes slashes, its escapes decoded', () => {
    expect(resolveTarget('ppt/slides', '../media/a.png?x#y')).toBe('ppt/media/a.png');
    expect(resolveTarget('ppt/slides', '..\\media\\a.png')).toBe('ppt/media/a.png');
    expect(resolveTarget('ppt/slides', '../media/%61.png')).toBe('ppt/media/a.png');
    expect(resolveTarget('ppt/slides', '../media/%E0%A4%A.png')).toBe('ppt/media/%E0%A4%A.png');
  });
});

describe('the answer of the worker is taken only in its shape', () => {
  it('a failure, for one of its reasons', () => {
    expect(readFromPptxWorker({ kind: 'failed', reason: 'too-large' })).toEqual({
      kind: 'failed',
      reason: 'too-large',
    });
    expect(readFromPptxWorker({ kind: 'failed', reason: 'other' })).toBeUndefined();
  });

  it('not parts with a field of another type', async () => {
    const answer = await readPresentation(raw(koreanPptx()));
    if (answer.kind !== 'read') throw new Error('not read');
    const slides = new Map<string, unknown>([['ppt/slides/slide1.xml', 42]]);
    expect(readFromPptxWorker({ ...answer, parts: { ...answer.parts, slides } })).toBeUndefined();
    expect(
      readFromPptxWorker({ ...answer, parts: { ...answer.parts, media: new Map([['a', 'b']]) } }),
    ).toBeUndefined();
    expect(readFromPptxWorker({ ...answer, characters: 1 })).toBeUndefined();
    expect(readFromPptxWorker({ ...answer, pixels: undefined })).toBeUndefined();
    expect(readFromPptxWorker({ ...answer, hidden: undefined })).toBeUndefined();
    expect(readFromPptxWorker({ ...answer, hidden: [1] })).toBeUndefined();
    expect(readFromPptxWorker({ ...answer, pixels: new Map([['x', -1]]) })).toBeUndefined();
    expect(readFromPptxWorker({ ...answer, pixels: new Map([['x', Infinity]]) })).toBeUndefined();
    expect(readFromPptxWorker(answer)).toEqual(answer);
    expect(readFromPptxWorker(null)).toBeUndefined();
  });
});
