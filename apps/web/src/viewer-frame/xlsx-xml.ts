/**
 * How the spreadsheet worker reads a workbook's XML parts: as a stream of
 * events (saxes), never as a tree, each part fed to the parser a slice at a
 * time. Names are taken by their local part (`x:c` is `c`), as the strict
 * and the transitional forms of the format differ only in namespaces.
 *
 * And the ceilings the worker holds a workbook to, each its own error, so
 * that a test can tell which one stopped a file.
 */
import { SaxesParser } from 'saxes';

import { SHEET_LIMIT, SHEET_NAME_LENGTH } from '$lib/file-viewer/protocol.ts';

import { TooMuch } from './worker-xml.ts';

/** The most sheets, and the longest name of one: what the viewer's protocol carries. */
export { SHEET_LIMIT, SHEET_NAME_LENGTH };

/** The most cells all the sheets of a workbook may have together: many times a long report's. */
export const CELL_LIMIT = 1_000_000;
/** The most strings (shared, inline, of formulas and errors) a workbook may have. */
export const STRING_LIMIT = 1_000_000;
/** The most characters those strings may have together. */
export const STRING_TEXT_LIMIT = 16_000_000;
/** The longest a cell's text is kept: Excel's own limit; past it the text is cut. */
export const STRING_LENGTH = 32_767;
/** The most cell styles, fonts, fills and borders together: Excel's limit is 64,000 styles. */
export const STYLE_LIMIT = 65_536;
/** The most number formats a workbook may define. */
export const FORMAT_LIMIT = 4_096;
/** The most merged ranges all the sheets may have together. */
export const MERGE_LIMIT = 100_000;
/**
 * The most rows of their own height (or hidden) all the sheets may have
 * together: Excel writes a height only for a row whose height was set.
 */
export const ROW_LIMIT = 250_000;
/** The most runs of columns (`col`) all the sheets may have together: a sheet has 16,384 columns. */
export const COLUMN_RUN_LIMIT = 16_384;

/** Thrown when the bytes are not a workbook this reads. */
export class Unreadable extends Error {}

/** Counts toward one ceiling; past it throws `TooMuch` with its name. */
export class Ceiling {
  readonly name: string;
  readonly #most: number;
  #count = 0;

  constructor(name: string, most: number) {
    this.name = name;
    this.#most = most;
  }

  get count(): number {
    return this.#count;
  }

  add(amount = 1): void {
    this.#count += amount;
    if (this.#count > this.#most) throw new TooMuch(this.name);
  }
}

/** The ceilings of one workbook. */
export function ceilings(): Record<
  'cells' | 'strings' | 'text' | 'styles' | 'formats' | 'merges' | 'sheets' | 'rows' | 'columns',
  Ceiling
> {
  return {
    cells: new Ceiling('cells', CELL_LIMIT),
    strings: new Ceiling('strings', STRING_LIMIT),
    text: new Ceiling('string text', STRING_TEXT_LIMIT),
    styles: new Ceiling('styles', STYLE_LIMIT),
    formats: new Ceiling('number formats', FORMAT_LIMIT),
    merges: new Ceiling('merged cells', MERGE_LIMIT),
    sheets: new Ceiling('sheets', SHEET_LIMIT),
    rows: new Ceiling('rows', ROW_LIMIT),
    columns: new Ceiling('columns', COLUMN_RUN_LIMIT),
  };
}

export type Attributes = Readonly<Record<string, string>>;

export interface Handlers {
  open?: (name: string, attributes: Attributes) => void;
  close?: (name: string) => void;
  text?: (text: string) => void;
}

/** How much of a part is fed to the parser at a time, in characters. */
const SLICE = 1 << 20;

const local = (name: string): string => name.slice(name.indexOf(':') + 1);

/** An attribute by its local name (`r:id` is `id`). */
export function attribute(attributes: Attributes, name: string): string | undefined {
  const plain = attributes[name];
  if (plain !== undefined) return plain;
  for (const [key, value] of Object.entries(attributes)) if (local(key) === name) return value;
  return undefined;
}

/** Reads a part's text as events; throws `Unreadable` when it is not well-formed XML. */
export function readXml(text: string, handlers: Handlers): void {
  const parser = new SaxesParser({ position: false });
  const { open, close, text: onText } = handlers;
  if (open) {
    parser.on('opentag', (tag) => {
      open(local(tag.name), tag.attributes as Attributes);
    });
  }
  if (close) {
    parser.on('closetag', (tag) => {
      close(local(tag.name));
    });
  }
  if (onText) {
    parser.on('text', onText);
    parser.on('cdata', onText);
  }
  let failed: Error | undefined;
  parser.on('error', (error) => {
    failed ??= error;
  });
  for (let at = 0; at < text.length && !failed; at += SLICE) {
    parser.write(text.slice(at, at + SLICE));
  }
  if (!failed) parser.close();
  if (failed) throw new Unreadable(failed.message);
}

/** Text with OOXML's `_xHHHH_` escapes undone (`_x000D_` is a carriage return). */
export function unescapeText(text: string): string {
  if (!text.includes('_x')) return text;
  return text.replace(/_x([0-9A-Fa-f]{4})_/g, (_, hex: string) =>
    String.fromCharCode(Number.parseInt(hex, 16)),
  );
}

/** A cell reference (`B12`) as its row and column, from 0; undefined when it is none or past a sheet's edge. */
export function cellAt(reference: string): [number, number] | undefined {
  const match = /^\$?([A-Za-z]{1,3})\$?(\d{1,7})$/.exec(reference);
  if (!match) return undefined;
  const [, letters = '', digits = ''] = match;
  let column = 0;
  for (const letter of letters.toUpperCase()) column = column * 26 + letter.charCodeAt(0) - 64;
  const row = Number(digits);
  if (column < 1 || column > 16_384 || row < 1 || row > 1_048_576) return undefined;
  return [row - 1, column - 1];
}

/** A whole number from an attribute, within `[least, most]`; undefined otherwise. */
export function whole(value: string | undefined, least: number, most: number): number | undefined {
  if (value === undefined || !/^\s*\d{1,10}\s*$/.test(value)) return undefined;
  const number = Number(value);
  return number >= least && number <= most ? number : undefined;
}

/** A finite number from an attribute, within `[least, most]`; undefined otherwise. */
export function decimal(
  value: string | undefined,
  least: number,
  most: number,
): number | undefined {
  if (value === undefined || value.trim() === '') return undefined;
  const number = Number(value);
  return Number.isFinite(number) && number >= least && number <= most ? number : undefined;
}

/** Whether an attribute says true (`1`, `true`), or `fallback` when it is not there. */
export function flag(value: string | undefined, fallback = false): boolean {
  if (value === undefined) return fallback;
  return value === '1' || value === 'true';
}
