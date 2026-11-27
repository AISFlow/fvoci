/**
 * What one XML part of a presentation would ask of the frame that draws
 * it, read in the PowerPoint worker (`pptx-worker.ts`) with saxes: how many
 * elements, how deep, how much text and how many tabs, how many shapes and
 * how deeply grouped, the cells of its tables, the pictures it shows and
 * which of them it recolours pixel by pixel, the SmartArt it draws, its
 * slide size if it has one, and whether it is a slide hidden from a show.
 *
 * What it cannot read as the renderer does, it counts more of: a picture is
 * any value of an attribute that names a picture's relationship, wherever
 * it is, and a table's cells are its `a:tc`, or its rows times its columns
 * when that is more. The renderer reads an attribute by its name with or
 * without a prefix (its `attr`), so every count here takes the worst of the
 * attributes of that local name: any that names a SmartArt, the largest
 * length, any slide side out of bounds.
 *
 * The frame parses the same text with the browser's `DOMParser`, so what is
 * read here is what is drawn: a part that is not well formed (with
 * namespaces, as the browser reads it) or declares a document type (whose
 * entities would make the browser's tree another than this one) is no part
 * the viewer reads.
 */
import { SaxesParser } from 'saxes';

import { UnreadableFile } from './renderer.ts';

/** The elements that are a shape on a slide, whatever their prefix. */
const SHAPES = new Set(['sp', 'pic', 'cxnSp', 'grpSp', 'graphicFrame', 'contentPart']);
/**
 * The attributes the renderer reads as a length in EMU (those it turns into
 * pixels: places, sizes, insets, margins, radii, column widths, row heights,
 * tab stops), whatever element has them.
 */
const LENGTHS = new Set([
  'x',
  'y',
  'cx',
  'cy',
  'w',
  'h',
  'dist',
  'blurRad',
  'rad',
  'tIns',
  'rIns',
  'lIns',
  'bIns',
  'marL',
  'marR',
  'marT',
  'marB',
  'indent',
  'defTabSz',
  'pos',
  'wR',
  'hR',
]);
/** The largest font size PowerPoint allows (4,000 points), in hundredths of a point. */
const FONT_SIZE = 400_000;
/**
 * The bounds of the other values the renderer turns into a size of text: a
 * spacing in points (`a:spcPts`, hundredths of a point, as a font size), a
 * spacing in lines (`a:spcPct`, thousandths of a percent: 100 lines), and
 * the scale autofit gives the text (`fontScale`, which only shrinks it).
 */
const TEXT_BOUNDS: Record<string, { attribute: string; most: number }> = {
  spcPts: { attribute: 'val', most: FONT_SIZE },
  spcPct: { attribute: 'val', most: 10_000_000 },
  normAutofit: { attribute: 'fontScale', most: 100_000 },
};
/** The effects of a picture the renderer works out pixel by pixel, on the frame's thread. */
const PIXEL_EFFECTS = new Set(['duotone', 'lum', 'biLevel']);
/** The longest attribute value kept as one that may name a relationship (a longer Id is left out). */
export const NAME_LENGTH = 255;
/** The slide size the renderer takes for a side the presentation does not give (its default), in EMU. */
const DEFAULT_SIZE = { cx: 9_144_000, cy: 6_858_000 };

/** A picture fill (`a:blip`): the values of the attributes in it, and how many pixel effects it has. */
export interface Blip {
  names: Set<string>;
  effects: number;
}

export interface PartMeasure {
  /** The length of the part's text: what the browser parses. */
  bytes: number;
  elements: number;
  depth: number;
  /** Characters of text. */
  text: number;
  /** Tab characters in the text and tab elements (`a:tab`): the renderer lays the text out once for each. */
  tabs: number;
  shapes: number;
  /** The shapes' text bodies, those of table cells left out: the renderer fits each to its box by measuring it. */
  textBodies: number;
  /** How deeply groups are nested in groups (0: no group). */
  groups: number;
  /** The cells of the tables, summed. */
  cells: number;
  /** How often each attribute value occurs (those up to `NAME_LENGTH` long): a relationship is named by one. */
  names: Map<string, number>;
  blips: Blip[];
  /** The SmartArt the part shows (`a:graphicData` naming a diagram): each draws a drawing of its own. */
  diagrams: number;
  /**
   * The largest length any attribute of `LENGTHS` gives, in EMU, either way
   * (a number past any, Infinity); or past it, a font size past `FONT_SIZE`.
   */
  extent: number;
  /** The slides the part lists (`p:sldId`), when it is the presentation. */
  slideIds: number;
  /** For each slide it lists, the values of its attributes named `id` (with a prefix or none). */
  slideRefs: string[][];
  /**
   * The sides the slide size (`p:sldSz` under the root) may be read as, in
   * EMU, when the part gives one: each `cx` and `cy` of it, or the renderer's
   * default for a side it gives no number for.
   */
  size?: { cx: number[]; cy: number[] };
  /** The characters of the text, each once. */
  characters: Set<string>;
  /** The root says it is not shown (a slide's `show`, false as the renderer reads it). */
  hidden: boolean;
}

interface Open {
  local: string;
  /** The picture fill this element is, when it is a `blip`. */
  blip?: Blip;
  /** The table this element is, when it is a `tbl`. */
  table?: { rows: number; columns: number; cells: number };
}

/** A length as the renderer reads it (`Number`), its size either way; no number is none. */
function length(value: string): number {
  const number = Math.abs(Number(value));
  return Number.isNaN(number) ? 0 : number;
}

/** The sides a slide size's attributes of one name may be read as: each number, or the default when none is. */
function sides(values: string[], fallback: number): number[] {
  const numbers = values.map(Number).filter((number) => !Number.isNaN(number));
  return numbers.length > 0 ? numbers : [fallback];
}

function emptyMeasure(bytes: number): PartMeasure {
  return {
    bytes,
    elements: 0,
    depth: 0,
    text: 0,
    tabs: 0,
    shapes: 0,
    textBodies: 0,
    groups: 0,
    cells: 0,
    names: new Map(),
    blips: [],
    diagrams: 0,
    extent: 0,
    slideIds: 0,
    slideRefs: [],
    characters: new Set(),
    hidden: false,
  };
}

type Tag = { local: string; attributes: Record<string, { local: string; value: string }> };

/** The values the renderer reads as false (its `pt`); any other is its default, true. */
const FALSE = new Set(['0', 'false', 'f', 'off']);

/**
 * Whether the root's `show` says it is not shown, read as the renderer
 * reads it (its `attr`): the attribute of that name, else the first of
 * that local name.
 */
function notShown(tag: Tag): boolean {
  const show =
    tag.attributes.show?.value ??
    Object.values(tag.attributes).find((each) => each.local === 'show')?.value;
  return show !== undefined && FALSE.has(show.trim().toLowerCase());
}

/** What an element adds to the measure, besides the counts every element has. */
/** Whether a text body is a table cell's: `a:tbl`, `a:tr`, `a:tc`, then it, as the renderer reads a table. */
function inCell(open: Open[]): boolean {
  const at = open.length;
  return (
    open[at - 1]?.local === 'tc' && open[at - 2]?.local === 'tr' && open[at - 3]?.local === 'tbl'
  );
}

function take(tag: Tag, open: Open[], measured: PartMeasure, element: Open): void {
  const { local } = tag;
  const attributes = Object.values(tag.attributes);
  /** Every value of the attributes of local name `name`: the renderer may read any of them. */
  const values = (name: string): string[] =>
    attributes.filter((each) => each.local === name).map((each) => each.value);
  const blip = open.findLast((each) => each.blip)?.blip;
  const bound = TEXT_BOUNDS[local];
  if (open.length === 0) measured.hidden = notShown(tag);
  for (const { local: name, value: named } of attributes) {
    if (LENGTHS.has(name)) measured.extent = Math.max(measured.extent, length(named));
    if (name === 'sz' && length(named) > FONT_SIZE) measured.extent = Infinity;
    if (name === bound?.attribute && length(named) > bound.most) measured.extent = Infinity;
    if (named.length > NAME_LENGTH) continue;
    measured.names.set(named, (measured.names.get(named) ?? 0) + 1);
    blip?.names.add(named);
  }
  if (local === 'blip') {
    element.blip = { names: new Set(attributes.map((each) => each.value)), effects: 0 };
    measured.blips.push(element.blip);
  } else if (PIXEL_EFFECTS.has(local) && blip) {
    blip.effects += 1;
  } else if (local === 'txBody' && !inCell(open)) {
    measured.textBodies += 1;
  } else if (local === 'tab' && open.at(-1)?.local !== 'tabLst') {
    // A tab of the text; not a tab stop, which the renderer lays nothing out for.
    measured.tabs += 1;
  } else if (local === 'tbl') {
    element.table = { rows: 0, columns: 0, cells: 0 };
  } else if (local === 'tr' || local === 'gridCol' || local === 'tc') {
    const table = open.findLast((each) => each.table)?.table;
    if (table) table[local === 'tr' ? 'rows' : local === 'gridCol' ? 'columns' : 'cells'] += 1;
  } else if (local === 'graphicData' && values('uri').some((uri) => uri.includes('diagram'))) {
    measured.diagrams += 1;
  } else if (local === 'sldId' && open.length === 2 && open[1]?.local === 'sldIdLst') {
    // The slide list under the root, as the renderer reads it; not a section's list of its slides.
    measured.slideIds += 1;
    measured.slideRefs.push(values('id'));
  } else if (local === 'sldSz' && open.length === 1 && !measured.size) {
    // The renderer reads the first under the root (its `child`), and a side that is no number as its default.
    measured.size = {
      cx: sides(values('cx'), DEFAULT_SIZE.cx),
      cy: sides(values('cy'), DEFAULT_SIZE.cy),
    };
  }
}

/** Measures one part; throws `UnreadableFile` when it is no XML the frame would read alike. */
export function measure(text: string): PartMeasure {
  const measured = emptyMeasure(text.length);
  const parser = new SaxesParser({ xmlns: true, position: false });
  const open: Open[] = [];
  let groups = 0;
  parser.on('doctype', () => {
    throw new UnreadableFile();
  });
  parser.on('opentag', (tag) => {
    const element: Open = { local: tag.local };
    measured.elements += 1;
    measured.depth = Math.max(measured.depth, open.length + 1);
    if (SHAPES.has(tag.local)) measured.shapes += 1;
    if (tag.local === 'grpSp') {
      groups += 1;
      measured.groups = Math.max(measured.groups, groups);
    }
    take(tag, open, measured, element);
    open.push(element);
  });
  parser.on('closetag', () => {
    const element = open.pop();
    if (element?.local === 'grpSp') groups -= 1;
    const table = element?.table;
    if (table) measured.cells += Math.max(table.cells, table.rows * table.columns);
  });
  const addText = (data: string): void => {
    measured.text += data.length;
    for (const character of data) {
      if (character === '\t') measured.tabs += 1;
      measured.characters.add(character);
    }
  };
  parser.on('text', addText);
  parser.on('cdata', addText);
  parser.on('error', () => {
    throw new UnreadableFile();
  });
  parser.write(text).close();
  return measured;
}
