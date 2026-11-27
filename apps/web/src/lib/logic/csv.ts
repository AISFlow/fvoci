/**
 * The rows of a view as a CSV file, as a spreadsheet opens it.
 *
 * The file is RFC 4180: cells between commas, lines ended by CRLF, a text in
 * double quotes with each of its own doubled. It starts with a byte order
 * mark, without which Excel reads UTF-8 as the code page of the system and
 * Korean comes out garbled.
 *
 * Anyone who may write on a board writes what a reader then opens in a
 * spreadsheet, which would run a text that starts like a formula (`=`, `+`,
 * `-`, `@`, a tab, a carriage return), also after spaces, line breaks or
 * invisible marks, and in the full width of East Asian text (`＝`, `＋`).
 * Such a text is written after an apostrophe, which the spreadsheet shows
 * as text, the apostrophe with it in some (a phone number `+82 …` too); a
 * plain number (`-5`) is no formula and stays a number.
 */
import { displayProperty } from './calendar.ts';
import type { DisplayContext } from './format.ts';
import { propertyBehavior, type ExportCell } from './properties.ts';
import { cardProperty, type Card, type PropertyTemplate, type ViewQuery } from './wire.ts';

/** The mark at the start of the file that says it is UTF-8. */
export const CSV_BOM = '﻿';

/** How a text a spreadsheet would take for a formula starts, once what it passes by is gone. */
const FORMULA_START = /^[=+\-@]/;
/** What a spreadsheet may pass by before a formula: spaces, line breaks, controls, invisible marks. */
const PASSED_BY = /^[\s\p{Cc}\p{Cf}]+/u;
/** A number as a spreadsheet reads one, with its sign. */
const PLAIN_NUMBER = /^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/i;

/**
 * A header row and one row per card, in the order given: the title, then the
 * visible properties in the order of the board. A calendar or a Gantt view
 * also exports the property it places its cards by (the one it names, or the
 * one it falls back to: `displayProperty`), shown on it or not.
 * `nameHeader` is the translated heading of the title column.
 */
export function tableRows(
  cards: readonly Card[],
  templates: readonly PropertyTemplate[],
  view: Pick<ViewQuery, 'viewType' | 'shownProperties' | 'dateProperty'>,
  nameHeader: string,
  context: DisplayContext,
): ExportCell[][] {
  const visible = templates.filter((template) => view.shownProperties.includes(template.id));
  if (view.viewType === 'calendar' || view.viewType === 'gantt') {
    const dateDisplay = displayProperty(view.dateProperty, templates);
    if (dateDisplay && !visible.includes(dateDisplay)) visible.push(dateDisplay);
  }

  const rows: ExportCell[][] = [[nameHeader, ...visible.map((template) => template.name)]];
  for (const card of cards) {
    const row: ExportCell[] = [card.title];
    for (const template of visible) {
      const value = cardProperty(card, template.id);
      row.push(propertyBehavior(template.type).exportValue(value, card, template, context));
    }
    rows.push(row);
  }
  return rows;
}

/** Whether a spreadsheet could run a text as a formula: it is read as the spreadsheet reads it. */
function runsAsFormula(text: string): boolean {
  if (text.startsWith('\t') || text.startsWith('\r')) return true;
  const head = text.normalize('NFKC').replace(PASSED_BY, '');
  return FORMULA_START.test(head) && !PLAIN_NUMBER.test(head);
}

/** One cell as the file has it: a number bare, a text guarded and quoted, nothing as nothing. */
export function csvCell(cell: ExportCell): string {
  if (typeof cell === 'number') return Number.isFinite(cell) ? String(cell) : '';
  if (cell === '') return '';
  const guarded = runsAsFormula(cell) ? `'${cell}` : cell;
  return `"${guarded.replaceAll('"', '""')}"`;
}

/** The rows as the text of a CSV file, its byte order mark first. */
export function csvText(rows: readonly (readonly ExportCell[])[]): string {
  return CSV_BOM + rows.map((row) => `${row.map(csvCell).join(',')}\r\n`).join('');
}
