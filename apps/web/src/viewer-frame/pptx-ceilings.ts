/**
 * The ceilings on what a presentation may ask of the frame, checked in the
 * PowerPoint worker before the frame gets anything (`pptx-read.ts`).
 *
 * The renderer draws with the frame's own layout: it measures text to fit
 * it to its box, so it cannot draw in a worker. The frame, whose thread the
 * app shares, draws one slide at a time and gives the thread back between
 * them (`pptx-view.ts`). So what has to be bounded is the work of one slide
 * and the work done before any is drawn:
 *
 * - before the first slide, the renderer parses the presentation, its
 *   themes, slide masters and layouts and every relationship part at once
 *   (their elements and their bytes are held together);
 * - a slide is drawn with its layout's and its master's shapes, and each
 *   SmartArt on any of them parses and draws a drawing of its own, so each
 *   ceiling of a slide holds for all of these together;
 * - a picture is decoded to its pixels, and one the slide recolours is
 *   worked out pixel by pixel on the frame's thread, once for each effect;
 * - the text is laid out once for each tab in it;
 * - a place or size beyond any slide's, or a slide size beyond what
 *   PowerPoint allows, is a lie of the file;
 * - a SmartArt drawing that shows SmartArt itself would have the renderer
 *   draw it inside itself without end.
 *
 * The relationships are the ones the worker wrote (`pptx-parts.ts`), which
 * the renderer reads as they are; what the slides' XML names is counted
 * generously instead (`pptx-xml.ts`). Past any ceiling the file is refused
 * as too large (`TooMuch`), before the frame draws anything: never once a
 * slide scrolls into view.
 */
import { resolveTarget, type Relationship, type Sorted } from './pptx-parts.ts';
import type { PartMeasure } from './pptx-xml.ts';
import { TooMuch } from './worker-xml.ts';

/** The largest slide side PowerPoint allows (56 inches), in EMU. */
const SLIDE_SIDE = 51_206_400;
/** The smallest slide side PowerPoint allows (1 inch), in EMU. */
export const SLIDE_SIDE_MIN = 914_400;

/** The ceilings of the whole presentation, by the name a deck past one is refused with. */
export const DECK_LIMITS = {
  /** Several times a long deck's slides. */
  slides: 1_000,
  'elements before the first slide': 300_000,
  'bytes before the first slide': 20_000_000,
  /** The largest slide side, in EMU (the smallest is `SLIDE_SIDE_MIN`). */
  'slide size': SLIDE_SIDE,
  /** The farthest place or largest size a shape may give, in EMU: ten times the largest slide's side. */
  extent: 10 * SLIDE_SIDE,
  /** The most pixels one picture may decode to (a 48-megapixel photograph has fewer). */
  picture: 50_000_000,
  /** SmartArt drawings that show SmartArt: none (PowerPoint writes none). */
  'SmartArt in SmartArt': 0,
} as const;

/**
 * The ceilings of one slide, with its layout, its master and its drawings.
 * The renderer took about a quarter of a second to draw a slide at any one
 * of `elements`, `shapes`, `textBodies`, `cells` or `text` (Chromium, on a
 * machine with other work); the time grows faster than the count past them
 * (2,000 text boxes took half a minute).
 */
export const SLIDE_LIMITS = {
  /** The XML parsed to draw it: the slide's, and its drawings' for each SmartArt. */
  bytes: 10_000_000,
  elements: 40_000,
  depth: 64,
  text: 100_000,
  tabs: 1_000,
  shapes: 1_000,
  /** Text boxes and shapes with text, which the renderer measures one by one. */
  textBodies: 250,
  groups: 16,
  cells: 1_000,
  pictures: 300,
  /** The pixels of the pictures it shows, each time it shows one. */
  pixels: 150_000_000,
  /** The pixels of the pictures it recolours, once for each effect. */
  recoloured: 4_000_000,
  /**
   * The share of its ceiling each of the five in `WORK` takes, summed: a
   * slide near several of them at once takes as long as several slides.
   */
  work: 2,
} as const;

/** What the drawing time of a slide grows with: the ceilings that `work` sums the shares of. */
const WORK = ['elements', 'shapes', 'textBodies', 'cells', 'text'] as const;

export const LIMITS = { ...DECK_LIMITS, ...SLIDE_LIMITS };
export type Limits = Record<keyof typeof LIMITS, number>;

/** What drawing one slide asks for, by the ceiling of a slide each is held to. */
export type SlideCount = Record<keyof typeof SLIDE_LIMITS, number>;

/** The name of a part's relationship part. */
const relationshipsOf = (part: string): string => {
  const at = part.lastIndexOf('/');
  return `${part.slice(0, at)}/_rels/${part.slice(at + 1)}.rels`;
};

const directoryOf = (part: string): string => part.slice(0, Math.max(0, part.lastIndexOf('/')));

/** The picture a relationship's target names, as the renderer looks it up (under `ppt/media/`). */
function mediaOf(target: string): string {
  const segments = target.split('/').filter(Boolean);
  const at = segments.lastIndexOf('media');
  const rest = at >= 0 && at < segments.length - 1 ? segments.slice(at + 1) : segments.slice(-1);
  return `ppt/media/${rest.join('/')}`;
}

class SlideCounter {
  readonly #sorted: Sorted;
  readonly count: SlideCount = {
    bytes: 0,
    elements: 0,
    depth: 0,
    text: 0,
    tabs: 0,
    shapes: 0,
    textBodies: 0,
    groups: 0,
    cells: 0,
    pictures: 0,
    pixels: 0,
    recoloured: 0,
    work: 0,
  };

  constructor(sorted: Sorted) {
    this.#sorted = sorted;
  }

  /** The pixels of the picture a relationship leads to, if it leads to one. */
  #area(relationship: Relationship): number {
    const pixels = this.#sorted.pixels.get(mediaOf(relationship.target));
    return pixels ? pixels.width * pixels.height : 0;
  }

  /**
   * Adds a part, drawn `times` times, its pictures named by `relationships`
   * (its own, or the slide's for a drawing). `parsed` is whether the part's
   * XML is parsed for the slide (not a layout's or master's, parsed before).
   */
  add(
    part: PartMeasure,
    relationships: readonly Relationship[],
    times: number,
    parsed: boolean,
  ): void {
    const { count } = this;
    if (parsed) count.bytes += part.bytes * times;
    count.elements += part.elements * times;
    count.depth = Math.max(count.depth, part.depth);
    count.text += part.text * times;
    count.tabs += part.tabs * times;
    count.shapes += part.shapes * times;
    count.textBodies += part.textBodies * times;
    count.groups = Math.max(count.groups, part.groups);
    count.cells += part.cells * times;
    const pictures = relationships.filter((each) => this.#area(each) > 0);
    for (const relationship of pictures) {
      const named = part.names.get(relationship.id) ?? 0;
      count.pictures += named * times;
      count.pixels += named * this.#area(relationship) * times;
    }
    for (const blip of part.blips) {
      if (blip.effects === 0) continue;
      const areas = pictures
        .filter((each) => blip.names.has(each.id))
        .map((each) => this.#area(each));
      count.recoloured += blip.effects * Math.max(0, ...areas) * times;
    }
  }

  /** The ceilings of a slide it passes. */
  over(limits: Limits): string[] {
    const { count } = this;
    count.work = WORK.reduce((sum, name) => sum + count[name] / limits[name], 0);
    return (Object.keys(SLIDE_LIMITS) as (keyof SlideCount)[]).filter(
      (name) => count[name] > limits[name],
    );
  }
}

/** The part that the first relationship of `part` the renderer takes for `type` leads to. */
function related(sorted: Sorted, part: string, type: string): string | undefined {
  // The renderer takes the first whose type has the name in it (its `Yi` and `Qs`).
  const found = sorted.relationships
    .get(relationshipsOf(part))
    ?.find((each) => each.type.includes(type));
  return found ? resolveTarget(directoryOf(part), found.target) : undefined;
}

interface SlideMeasure {
  over: string[];
  characters: Set<string>;
  count: SlideCount;
}

/** The layout and master a slide is drawn with, as the renderer finds them. */
export function drawnWith(sorted: Sorted, slide: string): { layout?: string; master?: string } {
  const layout = related(sorted, slide, 'slideLayout');
  return {
    layout,
    master: layout === undefined ? undefined : related(sorted, layout, 'slideMaster'),
  };
}

/** What drawing slide `slide` asks for: the ceilings of a slide it passes, and its characters. */
function measureSlide(sorted: Sorted, slide: string, limits: Limits): SlideMeasure {
  const counter = new SlideCounter(sorted);
  const { layout, master } = drawnWith(sorted, slide);
  const characters = new Set<string>();
  const take = (
    part: string,
    relationships: readonly Relationship[],
    times: number,
    parsed: boolean,
  ): void => {
    const measured = sorted.measures.get(part);
    if (!measured) return;
    counter.add(measured, relationships, times, parsed);
    for (const character of measured.characters) characters.add(character);
  };
  for (const part of [slide, layout, master]) {
    if (part === undefined) continue;
    const own = sorted.relationships.get(relationshipsOf(part)) ?? [];
    take(part, own, 1, part === slide);
    // Each SmartArt parses and draws one of the drawings its part names, and may parse another
    // for its layout: each of the part's drawings is counted twice for each.
    const diagrams = sorted.measures.get(part)?.diagrams ?? 0;
    if (diagrams === 0) continue;
    for (const { target } of own) {
      const drawing = resolveTarget(directoryOf(part), target);
      if (sorted.parts.diagramDrawings.has(drawing)) take(drawing, own, 2 * diagrams, true);
    }
  }
  return { over: counter.over(limits), characters, count: counter.count };
}

/** The parts the renderer parses before it draws any slide. */
function eagerParts(sorted: Sorted): string[] {
  const { parts } = sorted;
  return [
    '[Content_Types].xml',
    'ppt/presentation.xml',
    'ppt/_rels/presentation.xml.rels',
    'ppt/tableStyles.xml',
    ...[parts.themes, parts.slideMasters, parts.slideLayouts].flatMap((map) => [...map.keys()]),
    ...sorted.relationships.keys(),
  ];
}

/** The ceilings of the whole presentation it passes. */
function deckOver(sorted: Sorted, limits: Limits): string[] {
  const { measures, parts } = sorted;
  const over = new Set<string>();
  for (const measured of measures.values()) {
    if (measured.extent > limits.extent) over.add('extent');
  }
  for (const pixels of sorted.pixels.values()) {
    if (pixels.width * pixels.height > limits.picture) over.add('picture');
  }
  const nested = [...parts.diagramDrawings.keys()].filter(
    (drawing) => (measures.get(drawing)?.diagrams ?? 0) > 0,
  );
  if (nested.length > limits['SmartArt in SmartArt']) over.add('SmartArt in SmartArt');
  const presentation = measures.get('ppt/presentation.xml');
  const sides = presentation?.size ? [...presentation.size.cx, ...presentation.size.cy] : [];
  if (sides.some((each) => !(each >= SLIDE_SIDE_MIN && each <= limits['slide size']))) {
    over.add('slide size');
  }
  let elements = 0;
  let bytes = 0;
  for (const part of new Set(eagerParts(sorted))) {
    elements += measures.get(part)?.elements ?? 0;
    bytes += measures.get(part)?.bytes ?? 0;
  }
  if (elements > limits['elements before the first slide']) {
    over.add('elements before the first slide');
  }
  if (bytes > limits['bytes before the first slide']) over.add('bytes before the first slide');
  // The renderer takes the slides of the list, or every slide relationship when the list has none.
  const listed = presentation?.slideIds ?? 0;
  const slides = (sorted.relationships.get('ppt/_rels/presentation.xml.rels') ?? []).filter(
    (each) => each.type.endsWith('/slide'),
  );
  if (Math.max(listed, slides.length, parts.slides.size) > limits.slides) over.add('slides');
  return [...over];
}

/** What the presentation asks for, measured. */
export interface Measured {
  /** Every ceiling it passes, each once. */
  over: string[];
  /** The characters of all the slides' text, each once. */
  characters: string;
  /** What drawing each slide asks for, by the slide's name. */
  slides: Map<string, SlideCount>;
}

/** Measures the presentation against the ceilings `limits` (other ones only in tests). */
export function measureCeilings(sorted: Sorted, limits: Limits = LIMITS): Measured {
  const over = new Set(deckOver(sorted, limits));
  const characters = new Set<string>();
  const slides = new Map<string, SlideCount>();
  for (const slide of sorted.parts.slides.keys()) {
    const measured = measureSlide(sorted, slide, limits);
    for (const name of measured.over) over.add(name);
    for (const character of measured.characters) characters.add(character);
    slides.set(slide, measured.count);
  }
  return { over: [...over], characters: [...characters].join(''), slides };
}

/** Checks the presentation against every ceiling; throws `TooMuch` past one. */
export function checkCeilings(sorted: Sorted): Measured {
  const measured = measureCeilings(sorted);
  if (measured.over[0] !== undefined) throw new TooMuch(measured.over[0]);
  return measured;
}
