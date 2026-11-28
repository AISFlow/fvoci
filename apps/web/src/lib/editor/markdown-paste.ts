/**
 * Text pasted into a body that is Markdown: from a Markdown file, a README,
 * a chat, the output of a program in a terminal. It is read as Markdown
 * (markdown.ts), so that its headings, lists, tasks, quotes, code and tables
 * are those of the body and not their marks as letters.
 *
 * Text is taken for Markdown by its lines alone, and only text of two lines
 * or more: a line that starts a heading (`#` to `######` and a space), a
 * fence of code, a task (`- [ ]`, `- [x]`), a quote (`> `, a GitHub alert
 * among them: a callout), a toggle (`<details>`), two entries of a list, or
 * a table (a row of `|` and the row under its head). A single
 * line stays what it is, `# 3` among them. A number followed by another
 * (`2026. 11. 26. 회의`, a date as Korean writes it) starts no entry: such
 * lines are read as text, in a text that is Markdown for its other lines too.
 *
 * What the clipboard also has as HTML is pasted from the HTML, as the page it
 * came from drew it; this is for text alone. Text pasted as plain text
 * (Ctrl+Shift+V, ⌘⇧V) or into a code block is never read so (table-paste.ts,
 * which asks this). A shell script with `# comments` pasted into a paragraph
 * thus has headings: plain text is the way to paste it as it is.
 */
import type { Node as DocumentNode, Schema } from '@tiptap/pm/model';

import { markdownToBlocks } from './markdown.ts';

/** A cell of the line that parts a Markdown table's head from its body: `---`, `:--:`. */
const DELIMITER_CELL = /^:?-+:?$/;
/** A line that starts a heading. */
const HEADING = /^ {0,3}#{1,6}[ \t]+\S/m;
/** A line that opens or closes a fence of code. */
const FENCE = /^ {0,3}(`{3,}|~{3,})/m;
/** A line that is a task: the mark of an entry, then a box, open or ticked. */
const TASK = /^ {0,3}[-*+][ \t]+\[[ xX]\]([ \t]|$)/m;
/** A line that is quoted. */
const QUOTE = /^ {0,3}>[ \t]/m;
/** A line that opens a toggle. */
const DETAILS = /^ {0,3}<details(?:\s[^>]*)?>/im;
/**
 * A line that is an entry of a list: `-`, `*`, `+`, or a number and `.` or `)`, then a space; not
 * a number followed by another, which is a date (`2026. 11. 26.`).
 */
const ENTRY = /^ {0,3}([-*+]|\d{1,9}[.)](?![ \t]+\d{1,9}[.)]))[ \t]+\S/gm;
/** The number that starts a line and is followed by another: its mark, to be read as a letter. */
const NUMBER_BEFORE_NUMBER = /^( {0,3}\d{1,9})([.)])(?=[ \t]+\d{1,9}[.)])/gm;

/** Whether a line parts the head of a Markdown table from its body: `|---|:--:|`, read cell by cell. */
function partsTable(line: string): boolean {
  if (!/^ {0,3}[|:-]/.test(line)) return false;
  let cells = line.trim();
  if (cells.startsWith('|')) cells = cells.slice(1);
  if (cells.endsWith('|')) cells = cells.slice(0, -1);
  return cells.split('|').every((cell) => DELIMITER_CELL.test(cell.trim()));
}

/** Whether a text holds a Markdown table. */
function holdsTable(text: string): boolean {
  return text.includes('|') && text.split(/\r\n?|\n/).some(partsTable);
}

/**
 * Whether a text of more than one line is Markdown by what its lines start
 * with: a heading, a fence, a task, a quote, a toggle, two entries of a list. (A table
 * is told by what it is read as: `pastedMarkdown`.)
 */
export function looksLikeMarkdown(text: string): boolean {
  const lines = text.split(/\r\n?|\n/).filter((line) => line.trim() !== '');
  if (lines.length < 2) return false;
  if ([HEADING, FENCE, TASK, QUOTE, DETAILS].some((line) => line.test(text))) return true;
  return (text.match(ENTRY)?.length ?? 0) >= 2;
}

/** The blocks of a pasted text that is Markdown; undefined for any other text. */
export function pastedMarkdown(text: string, schema: Schema): DocumentNode[] | undefined {
  const marked = looksLikeMarkdown(text);
  if (!marked && !holdsTable(text)) return undefined;
  // `2026. 11. 26.` would be three lists, one in another: its first mark is escaped, a letter.
  const { content, notes } = markdownToBlocks(text.replace(NUMBER_BEFORE_NUMBER, '$1\\$2'));
  // What only looked like a table (`a | b` over a rule) and is read as none stays text;
  // a table past the limits is in it as its rows.
  const table = content.some((node) => node.type === 'table') || Boolean(notes.table);
  if ((!marked && !table) || content.length === 0) return undefined;
  return content.map((node) => schema.nodeFromJSON(node));
}
