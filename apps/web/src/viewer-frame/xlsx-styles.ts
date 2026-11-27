/**
 * A workbook's styles part (`xl/styles.xml`), read into what the frame
 * draws cells with: number format codes, fonts, fills (a colour; a pattern
 * other than solid is drawn as its colour mixed with the other), borders,
 * and the cell styles (`cellXfs`) that pick one of each with an alignment.
 *
 * Each table is held to `STYLE_LIMIT` entries as they are read, the formats
 * to `FORMAT_LIMIT`, and a format code to 255 characters; a style that
 * names an entry that is not there gets the first.
 */
import { colorOf, DEFAULT_PALETTE, type Palette } from './xlsx-colors.ts';
import {
  HORIZONTAL,
  LINES,
  rgbColor,
  VERTICAL,
  type Border,
  type Color,
  type Font,
  type Style,
} from './xlsx-protocol.ts';
import {
  attribute,
  flag,
  readXml,
  whole,
  decimal,
  type Attributes,
  type Ceiling,
} from './xlsx-xml.ts';

/** The longest format code kept; a longer one is drawn as `General`. */
/** A pattern's background when it names none: the sheet's white. */
const PAPER = rgbColor('ffffff');
const CODE_LENGTH = 255;
const FAMILY = /^[\p{L}\p{N} ._-]{1,64}$/u;

export interface Styles {
  /** The workbook's own format codes by number (`numFmtId`). */
  codes: Map<number, string>;
  fonts: Font[];
  fills: Color[];
  borders: Border[];
  /** The cell styles, with `format` the format's number (made an index by the reader). */
  styles: Style[];
  palette: string[];
}

const plainFont = (): Font => ({
  bold: false,
  italic: false,
  underline: 0,
  strike: false,
  size: 11,
  color: '',
  family: '',
});

const noBorder = (): Border => ({ left: [0, ''], right: [0, ''], top: [0, ''], bottom: [0, ''] });

/** Mixes two colours, `share` of the first. */
function mix(first: Color, second: Color, share: number): Color {
  if (!first) return second;
  if (!second) return first;
  const channel = (at: number): string => {
    const a = Number.parseInt(first.slice(at, at + 2), 16);
    const b = Number.parseInt(second.slice(at, at + 2), 16);
    return Math.round(a * share + b * (1 - share))
      .toString(16)
      .padStart(2, '0');
  };
  return `#${channel(1)}${channel(3)}${channel(5)}`;
}

/** How much of the foreground colour each pattern shows. */
const PATTERN_SHARE: Record<string, number> = {
  solid: 1,
  darkGray: 0.75,
  mediumGray: 0.5,
  lightGray: 0.25,
  gray125: 0.125,
  gray0625: 0.0625,
};

/** What is being read, element by element. */
interface Reading {
  font?: Font;
  fill?: { pattern: string; fg: Color; bg: Color; gradient: Color };
  border?: Border;
  side?: keyof Border;
  style?: { style: Style; fill: number };
  inXfs: boolean;
}

function fontPart(font: Font, name: string, attributes: Attributes, palette: Palette): void {
  const value = attribute(attributes, 'val');
  switch (name) {
    case 'b':
      font.bold = flag(value, true);
      break;
    case 'i':
      font.italic = flag(value, true);
      break;
    case 'strike':
      font.strike = flag(value, true);
      break;
    case 'u':
      font.underline = value === 'none' ? 0 : value?.startsWith('double') ? 2 : 1;
      break;
    case 'sz':
      font.size = decimal(value, 1, 409) ?? font.size;
      break;
    case 'color':
      font.color = colorOf(attributes, palette);
      break;
    case 'name':
      font.family = value !== undefined && FAMILY.test(value) ? value : '';
      break;
  }
}

function borderPart(
  reading: Reading,
  name: string,
  attributes: Attributes,
  palette: Palette,
): void {
  const border = reading.border;
  if (!border) return;
  const side = name === 'start' ? 'left' : name === 'end' ? 'right' : name;
  if (side === 'left' || side === 'right' || side === 'top' || side === 'bottom') {
    const style = LINES.indexOf(
      (attribute(attributes, 'style') ?? 'none') as (typeof LINES)[number],
    );
    border[side] = [Math.max(0, style), ''];
    reading.side = side;
  } else if (name === 'color' && reading.side) {
    const [style] = border[reading.side];
    border[reading.side] = [style, colorOf(attributes, palette)];
  } else if (name !== 'color') {
    // A line not drawn (a diagonal, a range's inner lines): its colour is no side's.
    reading.side = undefined;
  }
}

/** A cell style as its element gives it, and the number of its fill, put in once the fills are known. */
function cellStyle(attributes: Attributes): { style: Style; fill: number } {
  const id = (name: string): number => whole(attribute(attributes, name), 0, 0xffff) ?? 0;
  return {
    style: {
      format: id('numFmtId'),
      font: id('fontId'),
      fill: '',
      border: id('borderId'),
      horizontal: 0,
      vertical: 0,
      wrap: false,
      indent: 0,
    },
    fill: id('fillId'),
  };
}

function alignment(style: Style, attributes: Attributes): void {
  const across = attribute(attributes, 'horizontal') as (typeof HORIZONTAL)[number];
  const down = attribute(attributes, 'vertical') as (typeof VERTICAL)[number];
  style.horizontal = Math.max(0, HORIZONTAL.indexOf(across));
  style.vertical = Math.max(0, VERTICAL.indexOf(down));
  style.wrap = flag(attribute(attributes, 'wrapText'));
  style.indent = whole(attribute(attributes, 'indent'), 0, 250) ?? 0;
}

/**
 * The palette of `indexed` colours: the default, or the workbook's own
 * (`colors/indexedColors`), which comes after the styles that use it.
 */
function readPalette(text: string): string[] {
  const palette = [...DEFAULT_PALETTE];
  if (!text.includes('indexedColors')) return palette;
  let inside = false;
  let next = 0;
  readXml(text, {
    open: (name, attributes) => {
      if (name === 'indexedColors') inside = true;
      else if (inside && name === 'rgbColor') {
        const color = rgbColor(attribute(attributes, 'rgb'));
        if (next < 64 && color) palette[next] = color;
        next += 1;
      }
    },
    close: (name) => {
      if (name === 'indexedColors') inside = false;
    },
  });
  return palette;
}

/** Reads the styles part; `palette` is the theme's, and the part may redefine the indexed colours. */
export function readStyles(
  text: string,
  theme: readonly string[],
  counts: { styles: Ceiling; formats: Ceiling },
): Styles {
  const read: Styles = {
    codes: new Map(),
    fonts: [],
    fills: [],
    borders: [],
    styles: [],
    palette: readPalette(text),
  };
  const palette: Palette = { indexed: read.palette, theme };
  const reading: Reading = { inXfs: false };
  const fillIds: number[] = [];
  readXml(text, {
    open: (name, attributes) => {
      if (reading.font) fontPart(reading.font, name, attributes, palette);
      else if (reading.border) borderPart(reading, name, attributes, palette);
      else if (reading.fill) {
        if (name === 'patternFill')
          reading.fill.pattern = attribute(attributes, 'patternType') ?? 'none';
        else if (name === 'fgColor') reading.fill.fg = colorOf(attributes, palette);
        else if (name === 'bgColor') reading.fill.bg = colorOf(attributes, palette);
        else if (name === 'color' && !reading.fill.gradient)
          reading.fill.gradient = colorOf(attributes, palette);
      } else if (reading.style && name === 'alignment') alignment(reading.style.style, attributes);
      switch (name) {
        case 'numFmt': {
          counts.formats.add();
          const id = whole(attribute(attributes, 'numFmtId'), 0, 0xffff);
          const code = attribute(attributes, 'formatCode') ?? '';
          if (id !== undefined) read.codes.set(id, code.length > CODE_LENGTH ? 'General' : code);
          break;
        }
        case 'font':
          counts.styles.add();
          reading.font = plainFont();
          break;
        case 'fill':
          counts.styles.add();
          reading.fill = { pattern: 'none', fg: '', bg: '', gradient: '' };
          break;
        case 'border':
          counts.styles.add();
          reading.border = noBorder();
          break;
        case 'cellXfs':
          reading.inXfs = true;
          break;
        case 'xf':
          if (reading.inXfs) {
            counts.styles.add();
            reading.style = cellStyle(attributes);
          }
          break;
      }
    },
    close: (name) => {
      if (name === 'font' && reading.font) {
        read.fonts.push(reading.font);
        reading.font = undefined;
      } else if (name === 'fill' && reading.fill) {
        const { pattern, fg, bg, gradient } = reading.fill;
        const share = PATTERN_SHARE[pattern];
        read.fills.push(gradient || (pattern === 'none' ? '' : mix(fg, bg || PAPER, share ?? 0.5)));
        reading.fill = undefined;
      } else if (name === 'border' && reading.border) {
        read.borders.push(reading.border);
        reading.border = undefined;
        reading.side = undefined;
      } else if (name === 'cellXfs') reading.inXfs = false;
      else if (name === 'xf' && reading.style) {
        fillIds.push(reading.style.fill);
        read.styles.push(reading.style.style);
        reading.style = undefined;
      }
    },
  });
  if (read.fonts.length === 0) read.fonts.push(plainFont());
  if (read.borders.length === 0) read.borders.push(noBorder());
  read.styles.forEach((style, index) => {
    style.fill = read.fills[fillIds[index] ?? 0] ?? '';
    if (style.font >= read.fonts.length) style.font = 0;
    if (style.border >= read.borders.length) style.border = 0;
  });
  return read;
}
