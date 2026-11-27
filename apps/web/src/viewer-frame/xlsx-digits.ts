/**
 * A number drawn with the digits of a number format section (`#,##0.00`,
 * `0.0%`, `0.00E+00`, `000-0000`), and as `General` draws it.
 *
 * A spreadsheet keeps a number as a double and shows it to 15 significant
 * digits, rounding halves away from zero: so `1.005` with two decimals is
 * `1.01`, where JavaScript's `toFixed` gives `1.00`. The digits here come
 * from the 15-digit form (`toExponential(14)`), rounded as decimals.
 */
import type { Token } from './xlsx-codes.ts';

/** A non-negative number's whole part and its `decimals` decimal digits, rounded half away from zero. */
export function decimalParts(value: number, decimals: number): { whole: string; fraction: string } {
  const [mantissa = '0', exponent = '0'] = value.toExponential(14).split('e');
  const digits = BigInt(mantissa.replace('.', ''));
  // The digits are a 15-digit integer D; the value is D × 10^shift.
  const shift = Number(exponent) - 14 + decimals;
  let scaled: bigint;
  if (shift >= 0) scaled = digits * 10n ** BigInt(shift);
  else if (shift < -16) scaled = 0n;
  else {
    const unit = 10n ** BigInt(-shift);
    scaled = digits / unit + (2n * (digits % unit) >= unit ? 1n : 0n);
  }
  const text = scaled.toString().padStart(decimals + 1, '0');
  return {
    whole: text.slice(0, text.length - decimals).replace(/^0+/, ''),
    fraction: text.slice(text.length - decimals),
  };
}

/** The exponent's digits, at least `width` of them, with the sign the code asks for. */
function exponentText(exponent: number, width: number, sign: '+' | '-'): string {
  const digits = String(Math.abs(exponent)).padStart(width, '0');
  return exponent < 0 ? `-${digits}` : sign === '+' ? `+${digits}` : digits;
}

/** A number as `General` draws it in a column of standard width: 11 characters, or the exponent form. */
export function general(value: number): string {
  if (!Number.isFinite(value)) return '#NUM!';
  if (value === 0) return '0';
  const sign = value < 0 ? '-' : '';
  const size = Math.abs(value);
  const whole = Math.floor(size).toString().length;
  if (size >= 1e11 || size < 1e-9) return sign + scientific(size, 5);
  const { whole: integer, fraction } = decimalParts(size, Math.max(0, 10 - whole));
  const decimals = fraction.replace(/0+$/, '');
  if (integer === '' && decimals === '') return sign + scientific(size, 5);
  return sign + (integer === '' ? '0' : integer) + (decimals === '' ? '' : `.${decimals}`);
}

/** `1.23457E+11`: up to `decimals` decimals, without trailing zeros. */
function scientific(size: number, decimals: number): string {
  let exponent = Math.floor(Math.log10(size));
  let parts = decimalParts(size / 10 ** exponent, decimals);
  if (parts.whole.length > 1) {
    exponent += 1;
    parts = decimalParts(size / 10 ** exponent, decimals);
  }
  const fraction = parts.fraction.replace(/0+$/, '');
  return `${parts.whole || '0'}${fraction ? `.${fraction}` : ''}E${exponentText(exponent, 2, '+')}`;
}

/** The parts of a number section: where its digits go and what scales the value. */
interface Layout {
  whole: Token[];
  fraction: Token[];
  exponent: Token[];
  sign: '+' | '-' | undefined;
  grouped: boolean;
  /** Powers of 1000 the value is divided by (commas at the end of the digits). */
  thousands: number;
  percents: number;
}

/** Where each token of a number section belongs. */
function layout(tokens: readonly Token[]): Layout {
  const found: Layout = {
    whole: [],
    fraction: [],
    exponent: [],
    sign: undefined,
    grouped: false,
    thousands: 0,
    percents: 0,
  };
  let place: 'whole' | 'fraction' | 'exponent' = 'whole';
  let pendingCommas = 0;
  for (const token of tokens) {
    if (token.kind === 'percent') found.percents += 1;
    if (token.kind === 'point' && place === 'whole') place = 'fraction';
    else if (token.kind === 'exponent') {
      place = 'exponent';
      found.sign = token.sign;
    } else if (token.kind === 'comma' && place !== 'exponent') pendingCommas += 1;
    else if (token.kind === 'digit') {
      // A comma between digits groups the thousands; commas after the last divide.
      if (pendingCommas > 0 && place === 'whole' && found.whole.length > 0) found.grouped = true;
      pendingCommas = 0;
      found[place].push(token);
    }
  }
  found.thousands = pendingCommas;
  return found;
}

/** The digits of the whole part placed into its placeholders, from the right. */
function placeWhole(digits: string, placeholders: readonly Token[], grouped: boolean): string[] {
  const chars = placeholders.map((token) => (token.kind === 'digit' ? token.char : '#'));
  if (grouped) {
    const first = chars.indexOf('0');
    const least = first < 0 ? 0 : chars.length - first;
    const padded = digits.padStart(least, '0');
    const text = padded.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return chars.map((_, index) => (index === 0 ? text : ''));
  }
  const out = chars.map(() => '');
  let rest = digits;
  for (let index = chars.length - 1; index >= 0; index -= 1) {
    if (index === 0) {
      out[0] = rest !== '' ? rest : chars[0] === '0' ? '0' : chars[0] === '?' ? ' ' : '';
    } else if (rest !== '') {
      out[index] = rest.slice(-1);
      rest = rest.slice(0, -1);
    } else {
      out[index] = chars[index] === '0' ? '0' : chars[index] === '?' ? ' ' : '';
    }
  }
  return out;
}

/** The decimal digits placed into their placeholders: trailing zeros of `#` go, of `?` become spaces. */
function placeFraction(digits: string, placeholders: readonly Token[]): string[] {
  const chars = placeholders.map((token) => (token.kind === 'digit' ? token.char : '#'));
  const out = Array.from(digits);
  for (let index = chars.length - 1; index >= 0 && out[index] === '0'; index -= 1) {
    if (chars[index] === '#') out[index] = '';
    else if (chars[index] === '?') out[index] = ' ';
    else break;
  }
  return out;
}

/** The mantissa and exponent of a number for a code with `wholeDigits` placeholders before the point. */
function engineering(
  size: number,
  wholeDigits: number,
  decimals: number,
): { parts: { whole: string; fraction: string }; exponent: number } {
  if (size === 0) return { parts: decimalParts(0, decimals), exponent: 0 };
  // More than one placeholder before the point: the exponent is a multiple of their count.
  const step = Math.max(1, wholeDigits);
  let exponent = Math.floor(Math.floor(Math.log10(size)) / step) * step;
  let parts = decimalParts(size / 10 ** exponent, decimals);
  if (parts.whole.length > step) {
    exponent += step;
    parts = decimalParts(size / 10 ** exponent, decimals);
  }
  return { parts, exponent };
}

/** What goes where in a drawn number: one string per placeholder, and the whole digits without one. */
interface Placed {
  whole: string[];
  /** The whole part, when the code has no placeholder for it (`.00`, `%`). */
  orphan: string;
  fraction: string[];
  exponent: string[];
}

/** Draws a non-negative number with a number section's tokens. */
export function drawDigits(size: number, tokens: readonly Token[]): string {
  const found = layout(tokens);
  const scaled = (size * 100 ** found.percents) / 1000 ** found.thousands;
  const decimals = found.fraction.length;
  let parts = decimalParts(scaled, decimals);
  const exponent: string[] = [];
  if (found.sign) {
    const drawn = engineering(scaled, found.whole.length, decimals);
    parts = drawn.parts;
    // The exponent's sign and digits at its first placeholder, as wide as its placeholders ask.
    const text = exponentText(drawn.exponent, found.exponent.length, found.sign);
    found.exponent.forEach((_, index) => exponent.push(index === 0 ? text : ''));
  }
  const placed: Placed = {
    whole: placeWhole(parts.whole, found.whole, found.grouped),
    // A section of literals alone (`"-"`) draws them, and no digits.
    orphan:
      found.whole.length === 0 && found.fraction.length + found.exponent.length > 0
        ? parts.whole
        : '',
    fraction: placeFraction(parts.fraction, found.fraction),
    exponent,
  };
  return join(tokens, placed);
}

/** The section's tokens with the digits put into their placeholders. */
function join(tokens: readonly Token[], placed: Placed): string {
  let out = '';
  let place: 'whole' | 'fraction' | 'exponent' = 'whole';
  const used = { whole: 0, fraction: 0, exponent: 0 };
  let orphan = placed.orphan;
  const flush = (): void => {
    out += orphan;
    orphan = '';
  };
  for (const token of tokens) {
    if (token.kind === 'literal') out += token.text;
    else if (token.kind === 'digit') {
      if (place !== 'whole') flush();
      out += placed[place][used[place]] ?? '';
      used[place] += 1;
    } else if (token.kind === 'point') {
      flush();
      out += '.';
      if (place === 'whole') place = 'fraction';
    } else if (token.kind === 'exponent') {
      flush();
      out += 'E';
      place = 'exponent';
    } else if (token.kind === 'percent') {
      flush();
      out += '%';
    } else if (token.kind === 'slash') out += '/';
  }
  return out + orphan;
}
