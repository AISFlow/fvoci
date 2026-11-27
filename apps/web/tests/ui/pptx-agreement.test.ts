/**
 * The PowerPoint worker counts what the renderer will draw only if both find
 * the same parts: here the renderer builds its model (in the simulated page,
 * whose XML parser it uses) of the parts the worker answered with, and the
 * layout and master of each slide are the ones the worker counted it with
 * (src/viewer-frame/pptx-ceilings.ts), however the deck named them.
 */
import { buildPresentation, renderSlide } from '@aiden0z/pptx-renderer';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { drawnWith, measureCeilings, SLIDE_LIMITS } from '../../src/viewer-frame/pptx-ceilings.ts';
import { sortParts } from '../../src/viewer-frame/pptx-parts.ts';
import { unpack } from '../../src/viewer-frame/pptx-unpack.ts';
import { PIXELS_DRAWN } from '../../src/viewer-frame/pptx-view.ts';
import { repack } from '../../src/viewer-frame/zip-guard.ts';

import {
  DISGUISED_PPTX,
  tabbed,
  heavyPptx,
  layoutAndSlidePptx,
  smartArtPptx,
} from '../support/hostile-pptx.ts';
import { png } from '../support/viewer-files.ts';
import { deck, koreanPptx, NAMESPACES, picture, run, textBox } from '../support/viewer-pptx.ts';

/** A layout of the deck that the slide names in a way PowerPoint does not write. */
const twoLayouts = (type: string, target: string): Buffer =>
  deck({
    slides: [
      {
        shapes: '',
        layout: { id: 'rIdOther', type, target },
      },
    ],
    parts: [
      {
        name: 'slideLayouts/slideLayout2.xml',
        data: `<p:sldLayout ${NAMESPACES}><p:cSld><p:spTree/></p:cSld></p:sldLayout>`,
      },
    ],
  });

const DECKS: Record<string, () => Buffer> = {
  'the Korean sample': koreanPptx,
  'a layout named with a query': () => layoutAndSlidePptx('../slideLayouts/slideLayout1.xml?x#y'),
  'a layout named with backslashes': () => layoutAndSlidePptx('..\\slideLayouts\\slideLayout1.xml'),
  'a layout named with an escape': () => layoutAndSlidePptx('../slideLayouts/slide%4Cayout1.xml'),
  'a layout named from the root': () => layoutAndSlidePptx('/ppt/slideLayouts/./slideLayout1.xml'),
  'a layout of a type that only has the name in it': () =>
    twoLayouts('slideLayouts', '../slideLayouts/slideLayout2.xml'),
  'a layout that is not there': () => twoLayouts('slideLayout', '../slideLayouts/slideLayout9.xml'),
  SmartArt: () => smartArtPptx(3, 2),
};

describe('the renderer draws each slide with the layout and master the worker counted', () => {
  for (const [name, make] of Object.entries(DECKS)) {
    it(name, async () => {
      const sorted = sortParts(await unpack(await repack(Uint8Array.from(make()).buffer)));
      const presentation = buildPresentation(sorted.parts, { lazySlides: true });
      expect(presentation.slides.length).toBeGreaterThan(0);
      for (const [index, slide] of presentation.slides.entries()) {
        const { layout, master } = drawnWith(sorted, slide.slidePath);
        const drawnLayout = presentation.slideToLayout.get(index);
        expect(drawnLayout).toBe(layout);
        expect(
          drawnLayout === undefined ? undefined : presentation.layoutToMaster.get(drawnLayout),
        ).toBe(master);
      }
    });
  }
});

/** Decks under every ceiling, each near some, to be drawn. */
const DRAWN: Record<string, () => Buffer> = {
  'the Korean sample': koreanPptx,
  'SmartArt drawn by three frames': () => smartArtPptx(3, 2),
  'a slide of many text boxes': () => heavyPptx(1, 40),
  'tabs as characters and as elements': () =>
    deck({
      slides: [
        {
          shapes: textBox(2, [0, 0, 4_000_000, 1_000_000], [tabbed(2), run('가\t나')]),
        },
      ],
    }),
  'one picture shown many times, on the slide and on its layout': () =>
    deck({
      slides: [
        {
          shapes: Array.from({ length: 5 }, (_, index) =>
            picture(index + 2, 'rIdPicture', [0, 0, 1_000_000, 1_000_000]),
          ).join(''),
          links: [{ id: 'rIdPicture', type: 'image', target: '../media/image1.png' }],
        },
      ],
      layoutShapes: picture(2, 'rIdPicture', [0, 0, 1_000_000, 1_000_000]),
      layoutLinks: [{ id: 'rIdPicture', type: 'image', target: '../media/image1.png' }],
      parts: [{ name: 'media/image1.png', data: png(4, 4, [1, 2, 3]) }],
    }),
  // The decks in disguise, with the ceilings off: what the renderer draws of them is still counted.
  ...Object.fromEntries(
    Object.entries(DISGUISED_PPTX)
      .filter(([, { ceiling }]) => ceiling !== 'unreadable')
      .map(([name, { make }]) => [`in disguise: ${name}`, make]),
  ),
};

describe('what the renderer draws of a slide is no more than the worker counted', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  for (const [name, make] of Object.entries(DRAWN)) {
    it(name, async () => {
      const sorted = sortParts(await unpack(await repack(Uint8Array.from(make()).buffer)));
      const { slides } = measureCeilings(sorted);
      const presentation = buildPresentation(sorted.parts, { lazySlides: true });
      const parse = vi.spyOn(DOMParser.prototype, 'parseFromString');
      for (const slide of presentation.slides) {
        const counted = slides.get(slide.slidePath);
        if (!counted) throw new Error(`not counted: ${slide.slidePath}`);
        parse.mockClear();
        const handle = renderSlide(presentation, slide, {
          externalMedia: 'block',
          pdfjs: false,
          mediaUrlCache: new Map(),
        });
        // The slide's XML is parsed as it is drawn (its model is made lazily): the spy sees it.
        expect(parse).toHaveBeenCalled();
        const parsed = parse.mock.calls.reduce((sum, [text]) => sum + text.length, 0);
        const { element } = handle;
        expect(parsed, 'XML parsed').toBeLessThanOrEqual(counted.bytes);
        expect(element.querySelectorAll('img').length, 'pictures').toBeLessThanOrEqual(
          counted.pictures,
        );
        const tabs = element.querySelectorAll('[data-pptx-tab-stop]').length;
        expect(tabs, 'tabs laid out').toBeLessThanOrEqual(counted.tabs);
        // The deck of tabs lays some out: the check above can fail.
        if (name.startsWith('tabs')) expect(tabs).toBeGreaterThan(0);
        expect(element.querySelectorAll('*').length, 'elements').toBeLessThanOrEqual(
          // What the renderer makes of each element counted: a few of its own for each.
          10 * counted.elements + 100,
        );
        handle.dispose();
      }
    });
  }
});

it('the slides drawn at once show no more pixels together than one slide may', () => {
  expect(PIXELS_DRAWN).toBe(SLIDE_LIMITS.pixels);
});
