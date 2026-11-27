/**
 * Tables pasted into a body, and Markdown pasted as text.
 *
 * - Text that is Markdown (markdown-paste.ts: a table, a heading, a fence of
 *   code, tasks, a list, a quote) is read as Markdown, so that its table is a
 *   table and its heading a heading; other text is pasted as text, and so is
 *   any pasted as plain text (Ctrl+Shift+V) or into a code block.
 * - A table past 200 rows or 20 columns, from text or from a page, is pasted
 *   as its rows, each a line of its cells' text between ` | `, as the
 *   Markdown reader writes one out; a block of a cell with no text (a
 *   picture, a file, a formula, a rule) follows its row's line as itself.
 * - Into a table, a table pasted alone fills the cells from the one the
 *   reader is in (the table plugin's way). One that would take the table
 *   past a limit so, which the limits would refuse with nothing said, and
 *   one pasted with more around it, since no table stands in a cell, are
 *   pasted as their rows.
 */
import { Fragment, Slice, type Node as DocumentNode, type Schema } from '@tiptap/pm/model';
import { Plugin, PluginKey, type EditorState } from '@tiptap/pm/state';
import { cellAround, CellSelection, isInTable, TableMap } from '@tiptap/pm/tables';

import { pastedMarkdown } from './markdown-paste.ts';
import { MAX_TABLE_COLUMNS, MAX_TABLE_ROWS } from './table-grid.ts';

/** Whether a table is past 200 rows or 20 columns. */
export function isPastLimits(table: DocumentNode): boolean {
  const map = TableMap.get(table);
  return map.height > MAX_TABLE_ROWS || map.width > MAX_TABLE_COLUMNS;
}

/** What a parser of the clipboard gives back for what it leaves to the editor's own. */
const LEFT = undefined as unknown as Slice;

/**
 * A table's rows as lines: each the text of its cells between ` | `, each
 * cell's blocks between spaces, and after it the blocks of its cells that
 * hold no text, as they are.
 */
export function linesOf(table: DocumentNode, schema: Schema): DocumentNode[] {
  const lines: DocumentNode[] = [];
  const paragraph = schema.nodes.paragraph;
  if (!paragraph) return lines;
  table.forEach((row) => {
    const parts: DocumentNode[] = [];
    const blocks: DocumentNode[] = [];
    row.forEach((cell, _offset, index) => {
      if (index > 0) parts.push(schema.text(' | '));
      let first = true;
      cell.descendants((node) => {
        if (node.isBlock && node.isLeaf) blocks.push(node);
        if (!node.isTextblock) return !node.isLeaf;
        if (!first && node.childCount > 0) parts.push(schema.text(' '));
        if (node.childCount > 0) first = false;
        node.forEach((inline) => parts.push(inline));
        return false;
      });
    });
    if (parts.length > 0) lines.push(paragraph.create(null, parts));
    lines.push(...blocks);
  });
  return lines;
}

/**
 * Whether a table pasted alone at the reader's caret in a table would take it
 * past a limit: the table plugin fills the cells from the caret's, and adds
 * the rows and columns the pasted table needs past the last. Cells chosen
 * take only what fits in them.
 */
function growsPastLimits(state: EditorState, pasted: DocumentNode): boolean {
  if (state.selection instanceof CellSelection) return false;
  const $cell = cellAround(state.selection.$from);
  if (!$cell) return false;
  const { left, top } = TableMap.get($cell.node(-1)).findCell($cell.pos - $cell.start(-1));
  const size = TableMap.get(pasted);
  return left + size.width > MAX_TABLE_COLUMNS || top + size.height > MAX_TABLE_ROWS;
}

/** The pasted blocks with each table that may not stand as one given as its rows. */
function placed(content: Fragment, state: EditorState): Fragment {
  const inTable = isInTable(state);
  const alone = content.childCount === 1;
  const nodes: DocumentNode[] = [];
  content.forEach((node) => {
    const asRows =
      node.type.spec.tableRole === 'table' &&
      (isPastLimits(node) || (inTable && (!alone || growsPastLimits(state, node))));
    if (asRows) {
      nodes.push(...linesOf(node, state.schema));
    } else nodes.push(node);
  });
  return Fragment.fromArray(nodes);
}

export function tablePaste(): Plugin {
  return new Plugin({
    key: new PluginKey('tablePaste'),
    props: {
      clipboardTextParser(text, $context, plain, view) {
        if (plain || $context.parent.type.spec.code) return LEFT;
        const blocks = pastedMarkdown(text, view.state.schema);
        if (!blocks) return LEFT;
        // A paragraph at either end joins the line pasted into; a table stays whole.
        return Slice.maxOpen(Fragment.fromArray(blocks), false);
      },
      transformPasted(slice, view) {
        if (!slice.content.content.some((node) => node.type.spec.tableRole === 'table'))
          return slice;
        const content = placed(slice.content, view.state);
        if (content.eq(slice.content)) return slice;
        // Open no further than it was, nor than what it now holds can be.
        const open = Slice.maxOpen(content, false);
        return new Slice(
          content,
          Math.min(slice.openStart, open.openStart),
          Math.min(slice.openEnd, open.openEnd),
        );
      },
    },
  });
}
