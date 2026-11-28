/**
 * What a document body may contain.
 *
 * The body of a card or a comment is one document: a tree of nodes with
 * marks on its text, stored as JSON. The server accepts a body only when
 * every node, mark and attribute in it is on this list; so the editor must
 * not be able to produce anything else, and what it loads is checked the
 * same way. This file is that list as data, and a validator over it. The
 * editor's extensions (extensions.ts) are configured to match, and a test
 * holds the two together.
 */
import {
  CALLOUT_TONES,
  isAllowedHref,
  isBoolean,
  isIntegerBetween,
  isLanguage,
  isNonEmptyText,
  isOneOf,
  isText,
  isUserId,
  isBlockId,
  type AttributeRule,
} from './schema-rules.ts';
import { gridProblem, MAX_TABLE_COLUMNS, MAX_TABLE_ROWS, type GridProblem } from './table-grid.ts';

export {
  CALLOUT_TONES,
  DEFAULT_TONE,
  isAllowedHref,
  isBlockId,
  isCalloutTone,
  isFileId,
  isLanguage,
  isUserId,
  type CalloutTone,
} from './schema-rules.ts';

/**
 * The version of this list. A stored body carries the version it was written
 * under. Version 2 added the two formula nodes, `inlineMath` and `blockMath`;
 * version 3 the `reference` to a card or a page; version 4 the `file` block;
 * version 5 the table and its rows and cells; version 6 the toggle (`details`,
 * its `detailsSummary` and `detailsContent`) and the `callout`; version 7 the
 * board view (`boardView`), a view of a board shown live in the body.
 */
export const BODY_VERSION = 7;

/** The largest body, as the bytes of its JSON in UTF-8 (`storedSize`). */
export const MAX_BODY_BYTES = 8 * 1024 * 1024;
export const MAX_DEPTH = 60;
export const MAX_NODES = 100_000;
/**
 * The most board views a body shows: each is a board to load and follow
 * for whoever reads it (board-view.ts).
 */
export const MAX_BOARD_VIEWS = 10;
/**
 * The longest source of a formula, in UTF-16 code units like every other
 * limit on text here. A formula is drawn on the main thread of whoever opens
 * the body, so its source is kept short enough to be drawn quickly.
 */
export const MAX_LATEX = 4000;

export interface DocMark {
  type: string;
  attrs?: Record<string, unknown>;
}

export interface DocNode {
  type: string;
  attrs?: Record<string, unknown>;
  content?: DocNode[];
  marks?: DocMark[];
  text?: string;
}

interface AttributeSpec {
  rule: AttributeRule;
  required: boolean;
}

interface NodeSpec {
  attrs: Record<string, AttributeSpec>;
  /** The node types allowed directly inside; none for a leaf. */
  children: readonly string[];
  /** Fewest children the node must have. */
  minChildren: number;
}

const required = (rule: AttributeRule): AttributeSpec => ({ rule, required: true });
const optional = (rule: AttributeRule): AttributeSpec => ({ rule, required: false });

const BLOCKS = [
  'paragraph',
  'heading',
  'bulletList',
  'orderedList',
  'taskList',
  'blockquote',
  'codeBlock',
  'horizontalRule',
  'image',
  'file',
  'blockMath',
] as const;
const INLINE = ['text', 'hardBreak', 'mention', 'reference', 'inlineMath'] as const;
/**
 * A table, a toggle, a callout and a board view stand in the document and
 * in what a toggle hides alone, so that no table is in another (`table.ts`),
 * and a board view, as wide as the page, is in no list, quote or cell.
 */
const DOCUMENT_BLOCKS = [...BLOCKS, 'table', 'details', 'callout', 'boardView'] as const;
/** The two parts of a toggle, in their order: the line it shows, and what it hides. */
const DETAILS_PARTS = ['detailsSummary', 'detailsContent'] as const;
/** A cell spans one column and one row unless it says more, at most a whole table. */
const CELL_ATTRS = {
  colspan: optional(isIntegerBetween(1, MAX_TABLE_COLUMNS)),
  rowspan: optional(isIntegerBetween(1, MAX_TABLE_ROWS)),
};

export const NODE_SPECS: Record<string, NodeSpec> = {
  doc: { attrs: {}, children: DOCUMENT_BLOCKS, minChildren: 1 },
  paragraph: { attrs: {}, children: INLINE, minChildren: 0 },
  heading: { attrs: { level: required(isIntegerBetween(1, 3)) }, children: INLINE, minChildren: 0 },
  bulletList: { attrs: {}, children: ['listItem'], minChildren: 1 },
  orderedList: {
    attrs: { start: optional(isIntegerBetween(0, 1_000_000_000)) },
    children: ['listItem'],
    minChildren: 1,
  },
  listItem: { attrs: {}, children: BLOCKS, minChildren: 1 },
  taskList: { attrs: {}, children: ['taskItem'], minChildren: 1 },
  taskItem: { attrs: { checked: required(isBoolean) }, children: BLOCKS, minChildren: 1 },
  blockquote: { attrs: {}, children: BLOCKS, minChildren: 1 },
  codeBlock: { attrs: { language: optional(isLanguage) }, children: ['text'], minChildren: 0 },
  horizontalRule: { attrs: {}, children: [], minChildren: 0 },
  hardBreak: { attrs: {}, children: [], minChildren: 0 },
  image: {
    attrs: { fileId: required(isNonEmptyText(100)), alt: optional(isText(1000)) },
    children: [],
    minChildren: 0,
  },
  // A file block names its file; its name, size and kind are read where it is shown.
  file: { attrs: { fileId: required(isNonEmptyText(100)) }, children: [], minChildren: 0 },
  mention: { attrs: { userId: required(isUserId) }, children: [], minChildren: 0 },
  // A reference is the identifier of a card or a page; its title is read where it is shown.
  reference: { attrs: { id: required(isBlockId) }, children: [], minChildren: 0 },
  // A formula is its LaTeX source and nothing else; it is drawn where it is shown.
  inlineMath: {
    attrs: { latex: required(isNonEmptyText(MAX_LATEX)) },
    children: [],
    minChildren: 0,
  },
  blockMath: {
    attrs: { latex: required(isNonEmptyText(MAX_LATEX)) },
    children: [],
    minChildren: 0,
  },
  table: { attrs: {}, children: ['tableRow'], minChildren: 1 },
  // A row may have no cell of its own where the cells above cover it.
  tableRow: { attrs: {}, children: ['tableHeader', 'tableCell'], minChildren: 0 },
  tableHeader: { attrs: CELL_ATTRS, children: BLOCKS, minChildren: 1 },
  tableCell: { attrs: CELL_ATTRS, children: BLOCKS, minChildren: 1 },
  // Whether a toggle is open is each reader's own: it is no attribute (details.ts).
  details: { attrs: {}, children: DETAILS_PARTS, minChildren: DETAILS_PARTS.length },
  detailsSummary: { attrs: {}, children: INLINE, minChildren: 0 },
  detailsContent: { attrs: {}, children: DOCUMENT_BLOCKS, minChildren: 1 },
  callout: { attrs: { tone: required(isOneOf(CALLOUT_TONES)) }, children: BLOCKS, minChildren: 1 },
  // A board view names its view; what the view shows, and how, is read where it is shown.
  boardView: { attrs: { viewId: required(isBlockId) }, children: [], minChildren: 0 },
  text: { attrs: {}, children: [], minChildren: 0 },
};

export const MARK_SPECS: Record<string, Record<string, AttributeSpec>> = {
  bold: {},
  italic: {},
  strike: {},
  code: {},
  link: { href: required(isAllowedHref) },
};

/** The validation problem reported when a table exceeds its row or column limit. */
export const TABLE_TOO_LARGE_PROBLEM = `more than ${MAX_TABLE_ROWS} rows or ${MAX_TABLE_COLUMNS} columns`;

/** What a table's grid being wrong is written as. */
const GRID_PROBLEMS: Record<GridProblem, string> = {
  tooLarge: TABLE_TOO_LARGE_PROBLEM,
  noCell: 'no cell',
  unfilled: 'a cell spans past the table or over another, or a row is narrower than the table',
};

/** Nodes whose first child has to be a paragraph (the editor's own rule for list entries). */
const STARTS_WITH_PARAGRAPH = new Set(['listItem', 'taskItem']);

function checkAttributes(
  where: string,
  attrs: unknown,
  specs: Record<string, AttributeSpec>,
  problems: string[],
): void {
  if (
    attrs !== undefined &&
    (typeof attrs !== 'object' || attrs === null || Array.isArray(attrs))
  ) {
    problems.push(`${where}: attrs is not an object`);
    return;
  }
  const given = (attrs ?? {}) as Record<string, unknown>;
  for (const name of Object.keys(given)) {
    // `Object.hasOwn`: an attribute named `constructor` is not on the list.
    const spec = Object.hasOwn(specs, name) ? specs[name] : undefined;
    if (!spec) problems.push(`${where}: unknown attribute "${name}"`);
    else if (!spec.rule(given[name]))
      problems.push(`${where}: attribute "${name}" has an invalid value`);
  }
  for (const [name, spec] of Object.entries(specs)) {
    if (spec.required && !(name in given))
      problems.push(`${where}: attribute "${name}" is missing`);
  }
}

const CELLS = new Set(['tableCell', 'tableHeader']);

/**
 * Whether any text or member name of a value holds U+0000, which the
 * server's store cannot keep (PostgreSQL's `jsonb`) and so refuses.
 */
function holdsNul(value: unknown): boolean {
  if (typeof value === 'string') return value.includes('\u0000');
  if (Array.isArray(value)) return value.some(holdsNul);
  if (typeof value === 'object' && value !== null) {
    return Object.entries(value).some(
      ([name, inner]) => name.includes('\u0000') || holdsNul(inner),
    );
  }
  return false;
}

/**
 * The attributes of a mention in a body that is only shown. Whoever reads a
 * shared board with its read token alone gets every mention with an empty
 * `userId`: drawn as an unknown user, never
 * written back.
 */
const SHOWN_MENTION = { userId: required(isText(100)) };
/**
 * The attributes of a reference in a body that is only shown: the same
 * reader gets a reference to anything outside the board with an empty `id`,
 * drawn as a link that cannot be seen.
 */
const SHOWN_REFERENCE = { id: required(isText(100)) };
/**
 * The attributes of a board view in a body that is only shown: the same
 * reader gets every board view with an empty `viewId`, drawn as a view that
 * cannot be seen.
 */
const SHOWN_BOARD_VIEW = { viewId: required(isText(100)) };

/** The attributes a node of a body that is only shown may have, where they are not those it is written with. */
const SHOWN_ATTRS: Record<string, Record<string, AttributeSpec>> = {
  mention: SHOWN_MENTION,
  reference: SHOWN_REFERENCE,
  boardView: SHOWN_BOARD_VIEW,
};

export interface ValidateOptions {
  /**
   * The document is only shown, not written: a mention may name nobody
   * (an empty `userId`), a reference nothing (an empty `id`) and a board
   * view no view (an empty `viewId`), and
   * U+0000, which only the server's store cannot
   * keep, is drawn as the letter it is.
   */
  shown?: boolean;
}

/** The problems with a text node's marks, in their stored order. */
function checkTextMarks(marks: readonly unknown[], at: string): string[] {
  const problems: string[] = [];
  const names: string[] = [];
  for (const mark of marks) {
    if (typeof mark !== 'object' || mark === null || Array.isArray(mark)) {
      problems.push(`${at}: a mark is not an object`);
      continue;
    }
    const {
      type: markType,
      attrs: markAttrs,
      ...others
    } = mark as DocMark & Record<string, unknown>;
    const markName = typeof markType === 'string' ? markType : '(no type)';
    names.push(markName);
    const markSpec =
      typeof markType === 'string' && Object.hasOwn(MARK_SPECS, markType)
        ? MARK_SPECS[markType]
        : undefined;
    if (!markSpec) problems.push(`${at}: unknown mark "${markName}"`);
    else checkAttributes(`${at}@${markName}`, markAttrs, markSpec, problems);
    for (const key of Object.keys(others)) {
      problems.push(`${at}@${markName}: unknown member "${key}"`);
    }
  }
  if (new Set(names).size !== names.length) problems.push(`${at}: a mark is given twice`);
  return problems;
}

/** A member of a node, read without trusting its shape. */
function member(child: unknown, name: string): unknown {
  return typeof child === 'object' && child !== null
    ? (child as Record<string, unknown>)[name]
    : undefined;
}

/** What is wrong with how many and which children a node of `type` holds, as a whole. */
function contentProblems(
  at: string,
  type: string,
  spec: NodeSpec,
  children: readonly unknown[],
  problems: string[],
): void {
  if (children.length < spec.minChildren)
    problems.push(`${at}: needs at least ${spec.minChildren} child`);
  if (children.length > 0 && spec.children.length === 0)
    problems.push(`${at}: cannot have content`);
  if (STARTS_WITH_PARAGRAPH.has(type) && member(children[0], 'type') !== 'paragraph') {
    problems.push(`${at}: has to start with a paragraph`);
  }
  if (type === 'codeBlock' && children.some((child) => member(child, 'marks') !== undefined)) {
    problems.push(`${at}: text in a code block has no marks`);
  }
  const parts = children.map((child) => member(child, 'type'));
  if (type === 'details' && parts.join() !== DETAILS_PARTS.join()) {
    problems.push(`${at}: has to hold a summary and then a content`);
  }
}

/**
 * Everything that is wrong with a document; an empty list means the server
 * will take it. Checked are the kinds of nodes and marks, their attributes,
 * which node may be inside which, and the limits on depth and size.
 * With `shown`, what may be drawn though it may not be written.
 */
export function validateDoc(doc: unknown, options: ValidateOptions = {}): string[] {
  const problems: string[] = [];
  let count = 0;
  let boardViews = 0;

  function visit(
    node: unknown,
    where: string,
    depth: number,
    allowed: readonly string[] | undefined,
  ): void {
    if (problems.length >= 20) return;
    if (typeof node !== 'object' || node === null || Array.isArray(node)) {
      problems.push(`${where}: not a node`);
      return;
    }
    const { type, attrs, content, marks, text, ...rest } = node as DocNode &
      Record<string, unknown>;
    const typeName = typeof type === 'string' ? type : '(no type)';
    const at = `${where}/${typeName}`;
    count += 1;
    if (count > MAX_NODES) {
      problems.push(`more than ${MAX_NODES} nodes`);
      return;
    }
    if (depth > MAX_DEPTH) {
      problems.push(`${at}: deeper than ${MAX_DEPTH}`);
      return;
    }
    // `Object.hasOwn`: a type named `constructor` or `__proto__` is not on the list.
    const spec =
      typeof type === 'string' && Object.hasOwn(NODE_SPECS, type) ? NODE_SPECS[type] : undefined;
    if (!spec) {
      problems.push(`${where}: unknown node type "${typeName}"`);
      return;
    }
    const before = problems.length;
    if (allowed && !allowed.includes(type))
      problems.push(`${at}: not allowed inside ${where || 'the root'}`);
    if (type === 'boardView') boardViews += 1;
    for (const key of Object.keys(rest)) problems.push(`${at}: unknown member "${key}"`);
    const shownAttrs = options.shown ? SHOWN_ATTRS[type] : undefined;
    checkAttributes(at, attrs, shownAttrs ?? spec.attrs, problems);

    if (type === 'text') {
      if (typeof text !== 'string' || text.length === 0) problems.push(`${at}: text is empty`);
      // What is stored comes from elsewhere: nothing about its shape is taken on trust.
      // A member that is there is a list; `null` is not an empty one.
      const stated: unknown = marks;
      const given: unknown = stated ?? [];
      if (stated === null || !Array.isArray(given)) {
        problems.push(`${at}: marks is not a list`);
        return;
      }
      for (const problem of checkTextMarks(given, at)) problems.push(problem);
    } else {
      if (text !== undefined) problems.push(`${at}: only text nodes have text`);
      if (marks !== undefined) problems.push(`${at}: only text nodes have marks`);
    }

    const listed: unknown = content;
    const children: unknown = listed ?? [];
    if (listed === null || !Array.isArray(children)) {
      problems.push(`${at}: content is not a list`);
      return;
    }
    contentProblems(at, type, spec, children, problems);
    (children as unknown[]).forEach((child, index) => {
      visit(child, `${at}[${index}]`, depth + 1, spec.children);
    });
    // The grid of a table is judged when it and its parts are each allowed.
    const fits = type === 'table' && problems.length === before;
    const grid = fits ? gridProblem(node) : undefined;
    if (grid) problems.push(`${at}: ${GRID_PROBLEMS[grid]}`);
  }

  visit(doc, '', 1, ['doc']);
  // Counted over the whole document once all else of it is allowed, as the server counts.
  if (problems.length === 0 && boardViews > MAX_BOARD_VIEWS)
    problems.push(`more than ${MAX_BOARD_VIEWS} board views`);
  if (problems.length === 0 && !options.shown && holdsNul(doc))
    problems.push('a text or a name holds U+0000');
  return problems;
}

/** The document an empty body stands for. */
export function emptyDoc(): DocNode {
  return { type: 'doc', content: [{ type: 'paragraph' }] };
}

/** Whether a document is the one an empty body is stored as: one paragraph with nothing in it. */
export function isEmptyDoc(doc: DocNode): boolean {
  const content: unknown = doc.content;
  if (!Array.isArray(content) || content.length !== 1) return false;
  const only: unknown = content[0];
  if (typeof only !== 'object' || only === null) return false;
  if ((only as DocNode).type !== 'paragraph') return false;
  const inner: unknown = (only as DocNode).content;
  return inner === undefined || (Array.isArray(inner) && inner.length === 0);
}

/**
 * A document in the form that is stored: attributes without a value are left
 * out, and so is an attribute list that became empty. The editor gives every
 * attribute it knows a slot, `null` when unset; the stored form has no nulls.
 * The start of an ordered list at 1 is left out too, and so is a cell's span
 * of 1: a list without a start starts at 1, a cell without a span spans one,
 * and the server stores a body that way (its projection of a document edited
 * together leaves them out), so a body has one form.
 * Marks stay on text alone: the editor lets a line break or a mention inside
 * bold text carry the mark too (`<strong>a<br>b</strong>`), which means
 * nothing for a node without text and is not on the list.
 */
export function toStoredDoc(node: DocNode): DocNode {
  const stored: DocNode = { type: node.type };
  const attrs = Object.entries(node.attrs ?? {}).filter(
    ([name, value]) =>
      value !== null &&
      value !== undefined &&
      !(node.type === 'orderedList' && name === 'start' && value === 1) &&
      !(CELLS.has(node.type) && (name === 'colspan' || name === 'rowspan') && value === 1),
  );
  if (attrs.length > 0) stored.attrs = Object.fromEntries(attrs);
  if (node.content) stored.content = node.content.map(toStoredDoc);
  if (node.marks && node.type === 'text') {
    stored.marks = node.marks.map((mark) => {
      const markAttrs = Object.entries(mark.attrs ?? {}).filter(
        ([, value]) => value !== null && value !== undefined,
      );
      return markAttrs.length > 0
        ? { type: mark.type, attrs: Object.fromEntries(markAttrs) }
        : { type: mark.type };
    });
  }
  if (node.text !== undefined) stored.text = node.text;
  return stored;
}

/** The size of a document as it travels: the bytes of its JSON in UTF-8. */
export function storedSize(doc: DocNode): number {
  return new TextEncoder().encode(JSON.stringify(doc)).length;
}

/** The language a code block has to name for it to be drawn as a diagram. */
export const DIAGRAM_LANGUAGE = 'mermaid';
/**
 * The longest source of a diagram that is drawn. A longer one stays a code
 * block. This is no rule of the stored document: a code block of any length
 * is valid, and the server does not look at its language.
 */
export const MAX_DIAGRAM_SOURCE = 20_000;
