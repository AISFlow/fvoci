/**
 * Markdown into document nodes, one way.
 *
 * Bodies are stored as documents, not as Markdown, so nothing is ever
 * converted back and forth. Markdown comes in at three places: text written
 * by an older version (the built-in templates), text pasted as Markdown, and
 * imports. This reads it as GitHub flavoured Markdown in which a single line
 * break is a line break, and keeps a tally of
 * everything the document cannot say, so that a conversion can be judged by
 * numbers.
 */
import { marked, type Token, type Tokens } from 'marked';

import { alertOf, detailsParts, detailsRun, opensDetails } from './markdown-details.ts';
import { isAllowedHref, type DocMark, type DocNode } from './schema.ts';
import { MAX_TABLE_COLUMNS, MAX_TABLE_ROWS } from './table-grid.ts';

/** What was changed or lost, by kind, with the number of times. */
export type ConversionNotes = Record<string, number>;

export interface Conversion {
  content: DocNode[];
  notes: ConversionNotes;
}

/**
 * The kinds of note, and what each means for the reader of the result:
 *
 * - `html-as-text`: HTML in the text is shown as the characters it is, never
 *   run or drawn.
 * - `heading-level-clamped`: a heading of level 4 to 6 became level 3.
 * - `table`: a table past 200 rows or 20 columns, or one in a quote or a
 *   list entry (a table stands only in the document itself), became one
 *   paragraph per row, its cells' text between ` | `.
 * - `table-alignment-dropped`: a table kept its cells and lost how their
 *   text is aligned; a document has no alignment.
 * - `image-by-address`: an image given by address became a link with the
 *   image's description as its text; documents only show uploaded files.
 * - `link-dropped`: a link with a scheme that is not allowed kept its text
 *   and lost its target.
 * - `link-title-dropped`: the tooltip of a link is not kept.
 * - `marks-dropped-in-code`: inline code cannot also be bold, a link, etc.
 * - `mixed-task-list`: a list mixing tasks and plain entries became a plain
 *   list; the boxes are written out as `[ ]` and `[x]`.
 * - `ordered-task-list`: a numbered task list lost its numbers.
 * - `code-language-dropped`: the language of a code block was not a plain tag.
 * - `toggle`: a toggle (`<details>`) in a quote, a list entry or a callout,
 *   where none stands, became its summary's line and then what it hides; one
 *   in 28 others (`MAX_TOGGLES`) became the lines it is written in, as text.
 * - `callout`: a GitHub alert (`> [!NOTE]`) in a quote, a list entry or a
 *   callout became a quote of what it says.
 */
export const NOTE_KINDS = [
  'html-as-text',
  'heading-level-clamped',
  'table',
  'table-alignment-dropped',
  'image-by-address',
  'link-dropped',
  'link-title-dropped',
  'marks-dropped-in-code',
  'mixed-task-list',
  'ordered-task-list',
  'code-language-dropped',
  'toggle',
  'callout',
] as const;

/** How the text is read: GitHub's Markdown, a single line break a line break. */
const READING = { gfm: true, breaks: true } as const;

/**
 * How many toggles one may be in: 28, and a line of text in the innermost is
 * as deep as a body goes (`MAX_DEPTH`, each toggle two levels). One deeper
 * is its lines as text, read no further.
 */
export const MAX_TOGGLES = 28;

/** The letters the named character references Markdown is written with stand for. */
const NAMED: Record<string, string> = {
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
  nbsp: '\u00a0',
};

/**
 * Text with its character references read (`&lt;`, `&#32;`, `&#x3C;`), as
 * Markdown reads them outside code: the server writes a summary's `<` so. A
 * reference that names no letter stays as it is.
 */
export function withReferencesRead(text: string): string {
  return text.replace(/&(#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[a-zA-Z]+);/g, (whole, name: string) => {
    if (!name.startsWith('#')) return NAMED[name] ?? whole;
    const code =
      name[1] === 'x' || name[1] === 'X'
        ? Number.parseInt(name.slice(2), 16)
        : Number(name.slice(1));
    const valid = code > 0 && code <= 0x10ffff && (code < 0xd800 || code > 0xdfff);
    return valid ? String.fromCodePoint(code) : '\ufffd';
  });
}

export type NoteKind = (typeof NOTE_KINDS)[number];

class Converter {
  readonly notes: ConversionNotes = {};
  /** How many quotes, list entries and callouts the blocks read now are in. */
  #inside = 0;
  /** How many toggles the blocks read now are in. */
  #toggles = 0;

  /** What `read` gives, read as inside one more quote or list entry. */
  #within<T>(read: () => T): T {
    this.#inside += 1;
    try {
      return read();
    } finally {
      this.#inside -= 1;
    }
  }

  note(kind: NoteKind): void {
    this.notes[kind] = (this.notes[kind] ?? 0) + 1;
  }

  text(text: string, marks: readonly DocMark[]): DocNode[] {
    if (text === '') return [];
    return [marks.length > 0 ? { type: 'text', text, marks: [...marks] } : { type: 'text', text }];
  }

  inline(tokens: readonly Token[] | undefined, marks: readonly DocMark[]): DocNode[] {
    const nodes: DocNode[] = [];
    for (const token of tokens ?? []) {
      switch (token.type) {
        case 'text':
        case 'escape': {
          const inner = (token as Tokens.Text).tokens;
          const written = token.text as string;
          const text = token.type === 'text' ? withReferencesRead(written) : written;
          nodes.push(...(inner ? this.inline(inner, marks) : this.text(text, marks)));
          break;
        }
        case 'strong':
          nodes.push(
            ...this.inline((token as Tokens.Strong).tokens, withMark(marks, { type: 'bold' })),
          );
          break;
        case 'em':
          nodes.push(
            ...this.inline((token as Tokens.Em).tokens, withMark(marks, { type: 'italic' })),
          );
          break;
        case 'del':
          nodes.push(
            ...this.inline((token as Tokens.Del).tokens, withMark(marks, { type: 'strike' })),
          );
          break;
        case 'codespan':
          // The code mark stands alone: the editor does not combine it with others.
          if (marks.length > 0) this.note('marks-dropped-in-code');
          nodes.push(...this.text((token as Tokens.Codespan).text, [{ type: 'code' }]));
          break;
        case 'br':
          nodes.push({ type: 'hardBreak' });
          break;
        case 'link': {
          const link = token as Tokens.Link;
          if (link.title) this.note('link-title-dropped');
          const href = withReferencesRead(link.href);
          if (isAllowedHref(href)) {
            nodes.push(
              ...this.inline(link.tokens, withMark(marks, { type: 'link', attrs: { href } })),
            );
          } else {
            this.note('link-dropped');
            nodes.push(...this.inline(link.tokens, marks));
          }
          break;
        }
        case 'image': {
          const image = token as Tokens.Image;
          this.note('image-by-address');
          const label = image.text || image.href;
          const linked = isAllowedHref(image.href)
            ? withMark(marks, { type: 'link', attrs: { href: image.href } })
            : marks;
          nodes.push(...this.text(label, linked));
          break;
        }
        case 'html':
          this.note('html-as-text');
          nodes.push(...this.text((token as Tokens.HTML).text, marks));
          break;
        default:
          nodes.push(...this.text(token.raw, marks));
      }
    }
    return mergeText(nodes);
  }

  paragraph(tokens: readonly Token[] | undefined): DocNode[] {
    const content = this.inline(tokens, []);
    return content.length > 0 ? [{ type: 'paragraph', content }] : [];
  }

  /** The blocks of a list entry; an entry always starts with a paragraph. */
  item(tokens: readonly Token[], prefix = ''): DocNode[] {
    const blocks: DocNode[] = [];
    // Blocks one after another are read together: a toggle is written in several.
    let run: Token[] = [];
    const read = (): void => {
      const taken = run;
      run = [];
      if (taken.length > 0) blocks.push(...this.#within(() => this.blocks(taken)));
    };
    for (const token of tokens) {
      if (token.type === 'checkbox') continue;
      if (token.type !== 'text') {
        run.push(token);
        continue;
      }
      read();
      blocks.push(...this.paragraph((token as Tokens.Text).tokens ?? [token]));
    }
    read();
    if (blocks[0]?.type !== 'paragraph') blocks.unshift({ type: 'paragraph' });
    if (prefix !== '') {
      const first = blocks[0];
      if (first)
        first.content = mergeText([{ type: 'text', text: prefix }, ...(first.content ?? [])]);
    }
    return blocks;
  }

  list(list: Tokens.List): DocNode[] {
    if (list.items.length === 0) return [];
    const tasks = list.items.filter((item) => item.task).length;
    if (tasks === list.items.length) {
      if (list.ordered) this.note('ordered-task-list');
      return [
        {
          type: 'taskList',
          content: list.items.map((item) => ({
            type: 'taskItem',
            attrs: { checked: Boolean(item.checked) },
            content: this.item(item.tokens),
          })),
        },
      ];
    }
    if (tasks > 0) this.note('mixed-task-list');
    const content = list.items.map((item) => ({
      type: 'listItem',
      content: this.item(item.tokens, item.task ? (item.checked ? '[x] ' : '[ ] ') : ''),
    }));
    if (!list.ordered) return [{ type: 'bulletList', content }];
    const start = typeof list.start === 'number' ? list.start : 1;
    return [{ type: 'orderedList', attrs: { start }, content }];
  }

  /**
   * A table, its first row of headers; past the limits, or in a quote or a
   * list entry, a paragraph per row instead.
   */
  table(table: Tokens.Table): DocNode[] {
    const rows = [table.header, ...table.rows];
    const width = table.header.length;
    if (table.align.some((align) => align !== null)) this.note('table-alignment-dropped');
    if (this.#inside > 0 || rows.length > MAX_TABLE_ROWS || width > MAX_TABLE_COLUMNS) {
      this.note('table');
      return rows.flatMap((row) => {
        const content: DocNode[] = [];
        row.forEach((cell, index) => {
          if (index > 0) content.push({ type: 'text', text: ' | ' });
          content.push(...this.inline(cell.tokens, []));
        });
        const merged = mergeText(content);
        return merged.length > 0 ? [{ type: 'paragraph', content: merged }] : [];
      });
    }
    // Every row as wide as the first, as GitHub draws it.
    const cell = (row: Tokens.TableCell[], column: number, type: string): DocNode => {
      const content = this.inline(row[column]?.tokens, []);
      return {
        type,
        content: [content.length > 0 ? { type: 'paragraph', content } : { type: 'paragraph' }],
      };
    };
    return [
      {
        type: 'table',
        content: rows.map((row, index) => ({
          type: 'tableRow',
          content: Array.from({ length: width }, (_, column) =>
            cell(row, column, index === 0 ? 'tableHeader' : 'tableCell'),
          ),
        })),
      },
    ];
  }

  /**
   * The toggle that opens at `tokens[start]`, where one may stand; its lines
   * elsewhere. Gives the index of the last token it took.
   */
  #details(tokens: readonly Token[], start: number, nodes: DocNode[]): number {
    const run = detailsRun(tokens, start);
    if (this.#toggles >= MAX_TOGGLES) {
      this.note('toggle');
      for (const line of run.raw.split(/\r\n?|\n/)) {
        if (line.trim() !== '')
          nodes.push({ type: 'paragraph', content: this.text(line.trim(), []) });
      }
      return start + run.length - 1;
    }
    const { summary, body } = detailsParts(run.raw);
    const line = this.inline(marked.Lexer.lexInline(summary, READING), []);
    this.#toggles += 1;
    let hidden: DocNode[];
    try {
      hidden = this.blocks(marked.lexer(body, READING));
    } finally {
      this.#toggles -= 1;
    }
    if (this.#inside > 0) {
      this.note('toggle');
      if (line.length > 0) nodes.push({ type: 'paragraph', content: line });
      nodes.push(...hidden);
    } else {
      // What a toggle hides may be what the document holds: a table, a callout, a toggle.
      nodes.push({
        type: 'details',
        content: [
          line.length > 0 ? { type: 'detailsSummary', content: line } : { type: 'detailsSummary' },
          { type: 'detailsContent', content: hidden.length > 0 ? hidden : [{ type: 'paragraph' }] },
        ],
      });
    }
    return start + run.length - 1;
  }

  /** A quote: a callout where it is an alert and one may stand, else a quote. */
  #quote(quote: Tokens.Blockquote): DocNode[] {
    const alert = alertOf(quote);
    if (!alert) {
      const content = this.#within(() => this.blocks(quote.tokens));
      return content.length > 0 ? [{ type: 'blockquote', content }] : [];
    }
    const content = this.#within(() => this.blocks(marked.lexer(alert.body, READING)));
    if (this.#inside > 0) {
      this.note('callout');
      return content.length > 0 ? [{ type: 'blockquote', content }] : [];
    }
    const blocks = content.length > 0 ? content : [{ type: 'paragraph' }];
    return [{ type: 'callout', attrs: { tone: alert.tone }, content: blocks }];
  }

  blocks(tokens: readonly Token[]): DocNode[] {
    const nodes: DocNode[] = [];
    for (let index = 0; index < tokens.length; index += 1) {
      const token = tokens[index];
      if (!token) continue;
      if (opensDetails(token)) {
        index = this.#details(tokens, index, nodes);
        continue;
      }
      switch (token.type) {
        case 'space':
        case 'def':
          break;
        case 'paragraph':
          nodes.push(...this.paragraph((token as Tokens.Paragraph).tokens));
          break;
        case 'text':
          nodes.push(...this.paragraph((token as Tokens.Text).tokens ?? [token]));
          break;
        case 'heading': {
          const heading = token as Tokens.Heading;
          if (heading.depth > 3) this.note('heading-level-clamped');
          const content = this.inline(heading.tokens, []);
          nodes.push({
            type: 'heading',
            attrs: { level: Math.min(heading.depth, 3) },
            ...(content.length > 0 ? { content } : {}),
          });
          break;
        }
        case 'code': {
          const code = token as Tokens.Code;
          const language = (code.lang ?? '').trim().split(/\s+/)[0] ?? '';
          const valid = /^[A-Za-z0-9+#._-]{1,40}$/.test(language);
          if (language !== '' && !valid) this.note('code-language-dropped');
          nodes.push({
            type: 'codeBlock',
            ...(valid ? { attrs: { language } } : {}),
            ...(code.text !== '' ? { content: [{ type: 'text', text: code.text }] } : {}),
          });
          break;
        }
        case 'blockquote':
          nodes.push(...this.#quote(token as Tokens.Blockquote));
          break;
        case 'list':
          nodes.push(...this.list(token as Tokens.List));
          break;
        case 'hr':
          nodes.push({ type: 'horizontalRule' });
          break;
        case 'html':
          this.note('html-as-text');
          nodes.push({
            type: 'paragraph',
            content: this.text((token as Tokens.HTML).text.trim(), []),
          });
          break;
        case 'table':
          nodes.push(...this.table(token as Tokens.Table));
          break;
        default:
          nodes.push(...this.paragraph([{ type: 'text', raw: token.raw, text: token.raw }]));
      }
    }
    return nodes.filter(
      (node) => node.type !== 'paragraph' || node.content === undefined || node.content.length > 0,
    );
  }
}

function withMark(marks: readonly DocMark[], mark: DocMark): DocMark[] {
  return marks.some((existing) => existing.type === mark.type) ? [...marks] : [...marks, mark];
}

const sameMarks = (first: DocNode, second: DocNode): boolean =>
  JSON.stringify(first.marks ?? []) === JSON.stringify(second.marks ?? []);

/** Joins neighbouring text nodes that carry the same marks. */
function mergeText(nodes: DocNode[]): DocNode[] {
  const merged: DocNode[] = [];
  for (const node of nodes) {
    const last = merged.at(-1);
    if (last?.type === 'text' && node.type === 'text' && sameMarks(last, node)) {
      last.text = (last.text ?? '') + (node.text ?? '');
    } else {
      merged.push(node);
    }
  }
  return merged;
}

/** The block nodes a Markdown text stands for, and what could not be kept. */
export function markdownToBlocks(markdown: string): Conversion {
  const converter = new Converter();
  const tokens = marked.lexer(markdown, READING);
  return { content: converter.blocks(tokens), notes: converter.notes };
}

/** Adds the counts of `more` into `total`. */
export function addNotes(total: ConversionNotes, more: ConversionNotes): void {
  for (const [kind, count] of Object.entries(more)) total[kind] = (total[kind] ?? 0) + count;
}
