/**
 * How a cell looks in the frame: the text it shows (its value through its
 * number format, `xlsx-format.ts`), where the text lies, and its font,
 * fill and borders, from the workbook's styles.
 *
 * Every text goes into the page as text (`textContent`), and every style as
 * one property at a time (`style.setProperty`) from values of checked
 * shapes: a colour is `#rrggbb`, a size a number, a family a plain name.
 */
import { codeOf, NumberFormat, type Drawn } from './xlsx-format.ts';
import {
  BLANK,
  BOOLEAN,
  ERROR,
  HORIZONTAL,
  LINES,
  NUMBER,
  rgbColor,
  STRING,
  VERTICAL,
  type Border,
  type Sheet,
  type Style,
  type Workbook,
} from './xlsx-protocol.ts';

/** CSS pixels per point. */
export const PX_PER_POINT = 96 / 72;
const COLOR = /^#[0-9a-f]{6}$/;
/** A border's colour when it names none: Excel's automatic one. */
const AUTOMATIC = rgbColor('000000');
const FALLBACK_FAMILY = '"Noto Sans KR", sans-serif';

/** How each style of border line is drawn: its width in pixels and its CSS style. */
const LINE_LOOK: Record<(typeof LINES)[number], [number, string]> = {
  none: [0, 'none'],
  thin: [1, 'solid'],
  medium: [2, 'solid'],
  dashed: [1, 'dashed'],
  dotted: [1, 'dotted'],
  thick: [3, 'solid'],
  double: [3, 'double'],
  hair: [1, 'dotted'],
  mediumDashed: [2, 'dashed'],
  dashDot: [1, 'dashed'],
  mediumDashDot: [2, 'dashed'],
  dashDotDot: [1, 'dotted'],
  mediumDashDotDot: [2, 'dotted'],
  slantDashDot: [2, 'dashed'],
};

/** What a cell shows: its text, its colour if its format gives one, and where its text lies across it. */
export interface Shown {
  text: string;
  color?: string;
  /** `left`, `center`, `right` or `justify`. */
  align: string;
}

const PLAIN_STYLE: Style = {
  format: 0,
  font: 0,
  fill: '',
  border: 0,
  horizontal: 0,
  vertical: 0,
  wrap: false,
  indent: 0,
};

export class Looks {
  readonly #workbook: Workbook;
  readonly #language: string;
  readonly #formats = new Map<number, NumberFormat>();
  readonly #paints = new Map<number, { fill: boolean; border: boolean }>();

  constructor(workbook: Workbook, language: string) {
    this.#workbook = workbook;
    this.#language = language;
  }

  style(index: number): Style {
    return this.#workbook.styles[index] ?? PLAIN_STYLE;
  }

  #format(index: number): NumberFormat {
    let format = this.#formats.get(index);
    if (!format) {
      const given = this.#workbook.formats[index] ?? 0;
      const own = new Map<number, string>();
      const code = typeof given === 'string' ? given : codeOf(given, own, this.#language);
      const system = this.#workbook.date1904 ? 1904 : 1900;
      format = new NumberFormat(code, system, this.#language, this.#workbook.palette);
      this.#formats.set(index, format);
    }
    return format;
  }

  /** What the cell at `index` of a sheet shows. */
  shown(sheet: Sheet, index: number): Shown {
    const kind = sheet.kinds[index] ?? BLANK;
    const value = sheet.values[index] ?? 0;
    const style = this.style(sheet.styles[index] ?? 0);
    let drawn: Drawn;
    let natural = 'left';
    switch (kind) {
      case NUMBER:
        drawn = this.#format(style.format).number(value);
        natural = 'right';
        break;
      case STRING:
        drawn = this.#format(style.format).text(this.#workbook.strings[value] ?? '');
        break;
      case BOOLEAN:
        drawn = { text: value === 1 ? 'TRUE' : 'FALSE' };
        natural = 'center';
        break;
      case ERROR:
        drawn = { text: this.#workbook.strings[value] ?? '' };
        natural = 'center';
        break;
      default:
        drawn = { text: '' };
    }
    const across = HORIZONTAL[style.horizontal] ?? 'general';
    const align =
      across === 'general' || across === 'fill'
        ? natural
        : across === 'centerContinuous'
          ? 'center'
          : across === 'distributed'
            ? 'justify'
            : across;
    return drawn.color
      ? { text: drawn.text, color: drawn.color, align }
      : { text: drawn.text, align };
  }

  /** Puts a style's font, fill and alignment on a cell's element, at `zoom`. */
  dress(element: HTMLElement, style: Style, shown: Shown, zoom: number): void {
    const font = this.#workbook.fonts[style.font] ?? this.#workbook.fonts[0];
    const set = (name: string, value: string): void => {
      element.style.setProperty(name, value);
    };
    if (COLOR.test(style.fill)) set('background-color', style.fill);
    if (font) {
      set('font-size', `${(font.size * PX_PER_POINT * zoom).toFixed(2)}px`);
      if (font.bold) set('font-weight', '700');
      if (font.italic) set('font-style', 'italic');
      const lines = [font.underline > 0 ? 'underline' : '', font.strike ? 'line-through' : ''];
      const decoration = lines.filter(Boolean).join(' ');
      if (decoration) set('text-decoration-line', decoration);
      if (font.underline === 2) set('text-decoration-style', 'double');
      if (/^[\p{L}\p{N} ._-]{1,64}$/u.test(font.family)) {
        set('font-family', `"${font.family}", ${FALLBACK_FAMILY}`);
      }
    }
    const color = shown.color ?? font?.color ?? '';
    if (COLOR.test(color)) set('color', color);
    set('text-align', shown.align);
    const down = VERTICAL[style.vertical] ?? 'bottom';
    set(
      'justify-content',
      down === 'top' ? 'flex-start' : down === 'bottom' ? 'flex-end' : 'center',
    );
    if (style.indent > 0) {
      const room = `${(style.indent * 9 * zoom).toFixed(1)}px`;
      set(shown.align === 'right' ? 'padding-right' : 'padding-left', room);
    }
  }

  /** What a style paints on a cell without a value: a fill, a border. */
  #painted(index: number): { fill: boolean; border: boolean } {
    let painted = this.#paints.get(index);
    if (!painted) {
      const style = this.style(index);
      const border = this.#workbook.borders[style.border];
      const sides = border ? [border.left, border.right, border.top, border.bottom] : [];
      painted = { fill: COLOR.test(style.fill), border: sides.some(([line]) => line > 0) };
      this.#paints.set(index, painted);
    }
    return painted;
  }

  /** Whether a style paints anything on a cell without a value: a fill or a border. */
  paints(index: number): boolean {
    const { fill, border } = this.#painted(index);
    return fill || border;
  }

  /** Whether a style draws a border about a cell. */
  bordered(index: number): boolean {
    return this.#painted(index).border;
  }

  /** Puts a style's fill and borders on the element of a cell without a value. */
  paint(element: HTMLElement, style: Style): void {
    if (COLOR.test(style.fill)) element.style.setProperty('background-color', style.fill);
    const edges = this.edges(style);
    if (edges) element.append(edges);
  }

  /** The element that draws a style's borders over a cell, or undefined when it has none. */
  edges(style: Style): HTMLElement | undefined {
    const border: Border | undefined = this.#workbook.borders[style.border];
    if (!border) return undefined;
    const sides = (['left', 'right', 'top', 'bottom'] as const).filter(
      (side) => border[side][0] > 0,
    );
    if (sides.length === 0) return undefined;
    const element = document.createElement('div');
    element.className = 'xlsx-edges';
    for (const side of ['left', 'right', 'top', 'bottom'] as const) {
      const [line, color] = border[side];
      const [width, kind] = LINE_LOOK[LINES[line] ?? 'none'];
      if (width === 0) continue;
      element.style.setProperty(
        `border-${side}`,
        `${width}px ${kind} ${COLOR.test(color) ? color : AUTOMATIC}`,
      );
      // Over the line between two cells, so that the two cells' borders of one line are one.
      const out = side === 'left' || side === 'top' ? Math.ceil(width / 2) : Math.floor(width / 2);
      element.style.setProperty(side, `${-out}px`);
    }
    return element;
  }
}
