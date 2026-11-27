/**
 * What a Hangul document holds, as rhwp reads it: for the tests that a saved
 * document is the document (hwp-roundtrip.test.ts). Two documents are
 * compared by their pages, the text drawn on them, their tables (rows,
 * columns, and how the cells span them) and their pictures (each placed one,
 * and the bytes of every picture each page draws, inside a table's cell or
 * not), and by how many controls of each kind they have.
 *
 * Both sides are rhwp's reading: what rhwp does not read of a document at all
 * is on neither side, and is not found here.
 */
import { createHash } from 'node:crypto';

import type { HwpDocument } from '@rhwp/core';

export interface Content {
  pages: number;
  /** The text drawn on all pages, in order, its white space left out. */
  text: string;
  /** The document's text as rhwp writes it to a text file, whatever its pages: white space left out. */
  words: string;
  /** Each table once, in the document's order: its rows and columns, and its cells' spans. */
  tables: string[];
  /** Each picture once: its size as drawn, and what its bytes hash to. */
  pictures: string[];
  /** Page by page, what the bytes of every picture drawn there hash to, wherever it lies. */
  drawn: string[][];
  /** How many controls of each kind there are (sections, headers, tables, fields, …). */
  controls: Record<string, number>;
}

interface Placed {
  type?: string;
  w?: number;
  h?: number;
  rowCount?: number;
  colCount?: number;
  secIdx?: number;
  paraIdx?: number;
  controlIdx?: number;
  stableIndex?: number[];
  cells?: { row?: number; col?: number; rowSpan?: number; colSpan?: number }[];
}

const hashOf = (data: Uint8Array | string): string =>
  createHash('sha256').update(data).digest('hex').slice(0, 16);

/** A table's rows and columns, how many of its cells lie on the page, and how they span. */
function tableOf(placed: Placed): string {
  const cells = (placed.cells ?? []).map((cell) =>
    [cell.row, cell.col, cell.rowSpan, cell.colSpan].join(','),
  );
  return `${String(placed.rowCount)}x${String(placed.colCount)}:${String(cells.length)}:${hashOf(cells.join(';'))}`;
}

/** What the bytes of the pictures a page draws hash to, sorted. */
function drawnOn(document: HwpDocument, page: number): string[] {
  const { keys = [] } = JSON.parse(document.getPageSourceImageKeys(page)) as { keys?: string[] };
  return keys
    .map((key) => {
      try {
        return hashOf(document.getSourceImageBytes(key));
      } catch {
        return 'not-read';
      }
    })
    .sort();
}

function pictureOf(document: HwpDocument, placed: Placed): string {
  const size = `${String(placed.w)}x${String(placed.h)}`;
  try {
    const { secIdx = 0, paraIdx = 0, controlIdx = 0 } = placed;
    const data = document.getControlImageData(secIdx, paraIdx, '[]', controlIdx);
    return `${size}:${hashOf(data)}`;
  } catch {
    // One rhwp does not hand out by its place (inside a cell): its size alone.
    return `${size}:not-read`;
  }
}

export function contentOf(document: HwpDocument): Content {
  const pages = document.pageCount();
  const text: string[] = [];
  const tables = new Map<string, string>();
  const pictures = new Map<string, string>();
  const drawn: string[][] = [];
  for (let page = 0; page < pages; page += 1) {
    const layout = JSON.parse(document.getPageTextLayout(page)) as { runs?: { text?: string }[] };
    text.push((layout.runs ?? []).map((run) => run.text ?? '').join(''));
    drawn.push(drawnOn(document, page));
    const placed = JSON.parse(document.getPageControlLayout(page)) as { controls?: Placed[] };
    for (const control of placed.controls ?? []) {
      const key = JSON.stringify(
        control.stableIndex ?? [control.secIdx, control.paraIdx, control.controlIdx],
      );
      if (control.type === 'table' && !tables.has(key)) {
        tables.set(key, tableOf(control));
      }
      if (control.type === 'image' && !pictures.has(key)) {
        pictures.set(key, pictureOf(document, control));
      }
    }
  }
  const controls: Record<string, number> = {};
  for (const control of JSON.parse(document.getControls()) as { ctrlId?: string }[]) {
    const id = control.ctrlId ?? '?';
    controls[id] = (controls[id] ?? 0) + 1;
  }
  const whole = JSON.parse(document.getTextFileUnicode()) as unknown;
  return {
    pages,
    text: text.join('').replace(/\s+/g, ''),
    words: (typeof whole === 'string' ? whole : JSON.stringify(whole)).replace(/\s+/g, ''),
    tables: [...tables.values()],
    pictures: [...pictures.values()],
    drawn,
    controls,
  };
}

/** What of `before` is not in `after` as it was: the names of what differs, none when nothing does. */
export function differences(before: Content, after: Content): string[] {
  const same = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);
  const sorted = (controls: Record<string, number>): [string, number][] =>
    Object.entries(controls).sort(([a], [b]) => a.localeCompare(b));
  return [
    ...(before.pages === after.pages ? [] : ['pages']),
    ...(before.text === after.text && before.words === after.words ? [] : ['text']),
    ...(same(before.tables, after.tables) ? [] : ['tables']),
    ...(same(before.pictures, after.pictures) && same(before.drawn, after.drawn)
      ? []
      : ['pictures']),
    ...(same(sorted(before.controls), sorted(after.controls)) ? [] : ['controls']),
  ];
}

/**
 * What of `before` is not in `after`, whatever the pages either lies on: for a document
 * edited, whose lines an edit moves, onto other pages too. Its text as a text file has it,
 * its tables by their rows and columns, its pictures by their bytes, its controls by kind.
 */
export function lost(before: Content, after: Content): string[] {
  const same = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);
  const tables = (content: Content): string[] =>
    content.tables.map((table) => table.slice(0, table.indexOf(':'))).sort();
  const pictures = (content: Content): string[] =>
    [
      ...new Set([
        ...content.pictures.map((one) => one.slice(one.indexOf(':') + 1)),
        ...content.drawn.flat(),
      ]),
    ].sort();
  const kinds = (content: Content): [string, number][] =>
    Object.entries(content.controls).sort(([a], [b]) => a.localeCompare(b));
  return [
    ...(before.words === after.words ? [] : ['text']),
    ...(same(tables(before), tables(after)) ? [] : ['tables']),
    ...(same(pictures(before), pictures(after)) ? [] : ['pictures']),
    ...(same(kinds(before), kinds(after)) ? [] : ['controls']),
  ];
}
