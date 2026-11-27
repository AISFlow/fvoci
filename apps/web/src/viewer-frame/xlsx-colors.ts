/**
 * The colours of a workbook, as `#rrggbb`: given as ARGB (`FF1F4E79`), as
 * an index into the palette (`indexed="10"`, the old 64 colours, which a
 * workbook may redefine), or as one of its theme's colours lightened or
 * darkened (`theme="4" tint="0.4"`).
 *
 * The theme's colours are read from its part (`a:clrScheme`) in the order
 * a cell names them, which is not the part's: the first two pairs swap
 * (0 is "light 1", 1 "dark 1", 2 "light 2", 3 "dark 2"), then the six
 * accents and the two link colours.
 */
import { rgbColor } from './xlsx-protocol.ts';
import { attribute, readXml, type Attributes } from './xlsx-xml.ts';

/** The default palette of `indexed` colours (BIFF8's), 0 to 63; 64 is the text's own colour. */
export const DEFAULT_PALETTE = [
  '000000',
  'ffffff',
  'ff0000',
  '00ff00',
  '0000ff',
  'ffff00',
  'ff00ff',
  '00ffff',
  '000000',
  'ffffff',
  'ff0000',
  '00ff00',
  '0000ff',
  'ffff00',
  'ff00ff',
  '00ffff',
  '800000',
  '008000',
  '000080',
  '808000',
  '800080',
  '008080',
  'c0c0c0',
  '808080',
  '9999ff',
  '993366',
  'ffffcc',
  'ccffff',
  '660066',
  'ff8080',
  '0066cc',
  'ccccff',
  '000080',
  'ff00ff',
  'ffff00',
  '00ffff',
  '800080',
  '800000',
  '008080',
  '0000ff',
  '00ccff',
  'ccffff',
  'ccffcc',
  'ffff99',
  '99ccff',
  'ff99cc',
  'cc99ff',
  'ffcc99',
  '3366ff',
  '33cccc',
  '99cc00',
  'ffcc00',
  'ff9900',
  'ff6600',
  '666699',
  '969696',
  '003366',
  '339966',
  '003300',
  '333300',
  '993300',
  '993366',
  '333399',
  '333333',
].map((hex) => `#${hex}`);

/** The theme's colours as cells name them, by default Office's. */
export const DEFAULT_THEME = [
  'ffffff',
  '000000',
  'e7e6e6',
  '44546a',
  '4472c4',
  'ed7d31',
  'a5a5a5',
  'ffc000',
  '5b9bd5',
  '70ad47',
  '0563c1',
  '954f72',
].map((hex) => `#${hex}`);

/** Where each colour of `a:clrScheme` goes in the order cells name them. */
const SCHEME_ORDER = [
  'lt1',
  'dk1',
  'lt2',
  'dk2',
  'accent1',
  'accent2',
  'accent3',
  'accent4',
  'accent5',
  'accent6',
  'hlink',
  'folHlink',
];

/** The theme part's colours, in the order cells name them; the default for any it does not give. */
export function readTheme(text: string): string[] {
  const found = [...DEFAULT_THEME];
  let slot = -1;
  let depth = 0;
  readXml(text, {
    open: (name, attributes) => {
      if (name === 'clrScheme') depth += 1;
      else if (depth > 0 && slot < 0) slot = SCHEME_ORDER.indexOf(name);
      else if (slot >= 0) {
        const color =
          name === 'srgbClr'
            ? rgbColor(attribute(attributes, 'val'))
            : name === 'sysClr'
              ? rgbColor(attribute(attributes, 'lastClr'))
              : '';
        if (color) found[slot] = color;
      }
    },
    close: (name) => {
      if (name === 'clrScheme') depth -= 1;
      else if (slot >= 0 && SCHEME_ORDER[slot] === name) slot = -1;
    },
  });
  return found;
}

/** Lightens (`tint` > 0) or darkens a colour as a spreadsheet does: in HSL, on its lightness. */
export function tinted(color: string, tint: number): string {
  if (!color || tint === 0 || !Number.isFinite(tint)) return color;
  const [r, g, b] = [1, 3, 5].map((at) => Number.parseInt(color.slice(at, at + 2), 16) / 255);
  const red = r ?? 0;
  const green = g ?? 0;
  const blue = b ?? 0;
  const high = Math.max(red, green, blue);
  const low = Math.min(red, green, blue);
  let light = (high + low) / 2;
  const chroma = high - low;
  const saturation = chroma === 0 ? 0 : chroma / (1 - Math.abs(2 * light - 1));
  let hue = 0;
  if (chroma !== 0) {
    if (high === red) hue = ((green - blue) / chroma + 6) % 6;
    else if (high === green) hue = (blue - red) / chroma + 2;
    else hue = (red - green) / chroma + 4;
  }
  const amount = Math.max(-1, Math.min(1, tint));
  light = amount < 0 ? light * (1 + amount) : light * (1 - amount) + amount;
  const c = (1 - Math.abs(2 * light - 1)) * saturation;
  const x = c * (1 - Math.abs((hue % 2) - 1));
  const m = light - c / 2;
  const [r1, g1, b1] =
    hue < 1
      ? [c, x, 0]
      : hue < 2
        ? [x, c, 0]
        : hue < 3
          ? [0, c, x]
          : hue < 4
            ? [0, x, c]
            : hue < 5
              ? [x, 0, c]
              : [c, 0, x];
  const hex = (value: number): string =>
    Math.round(Math.max(0, Math.min(1, value + m)) * 255)
      .toString(16)
      .padStart(2, '0');
  return `#${hex(r1)}${hex(g1)}${hex(b1)}`;
}

/** The colours a workbook's colour elements are read against. */
export interface Palette {
  indexed: readonly string[];
  theme: readonly string[];
}

/** The colour a colour element (`color`, `fgColor`) gives; '' for none, automatic or the system's. */
export function colorOf(attributes: Attributes, palette: Palette): string {
  if (attribute(attributes, 'auto') === '1' || attribute(attributes, 'auto') === 'true') return '';
  const tint = Number(attribute(attributes, 'tint') ?? '0');
  const rgb = attribute(attributes, 'rgb');
  if (rgb !== undefined) return tinted(rgbColor(rgb), tint);
  const theme = attribute(attributes, 'theme');
  if (theme !== undefined && /^\d{1,2}$/.test(theme)) {
    return tinted(palette.theme[Number(theme)] ?? '', tint);
  }
  const indexed = attribute(attributes, 'indexed');
  if (indexed !== undefined && /^\d{1,3}$/.test(indexed)) {
    return tinted(palette.indexed[Number(indexed)] ?? '', tint);
  }
  return '';
}
