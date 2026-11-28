/**
 * What a body says apart from its structure: its words, for search,
 * previews and notifications, and its tasks, for the badge on a card. The
 * server reads the same words from a body (`crates/model/src/allowlist.rs`),
 * and the shared vectors hold the two together.
 */
import type { DocNode } from './schema.ts';

/** How many tasks a document has and how many are ticked, for the badge on a card. */
export function countTasks(node: DocNode): { total: number; checked: number } {
  const count = { total: 0, checked: 0 };
  if (node.type === 'taskItem') {
    count.total += 1;
    if (node.attrs?.checked === true) count.checked += 1;
  }
  for (const child of node.content ?? []) {
    const inner = countTasks(child);
    count.total += inner.total;
    count.checked += inner.checked;
  }
  return count;
}

/** The blocks that are a line each; a toggle's summary is one, whether it is open or not. */
const TEXT_BLOCKS = new Set(['paragraph', 'heading', 'codeBlock', 'detailsSummary']);

const latexOf = (node: DocNode): string =>
  typeof node.attrs?.latex === 'string' ? node.attrs.latex : '';

/**
 * The words of a document without its structure, one line per paragraph:
 * for search, previews and notifications. `mentionLabel` names a mentioned
 * user; a reference gives nothing, as on the server. A formula gives its source: inside a line where it stands in one, on
 * a line of its own when it is a block. A diagram is a code block and gives
 * its source like any other code. A toggle gives its summary as a line,
 * then what it hides: folded is how a reader sees it, not what it says.
 */
export function plainText(
  node: DocNode,
  mentionLabel: (userId: string) => string = (id) => id,
): string {
  const lines: string[] = [];
  function visit(current: DocNode): string {
    if (current.type === 'text') return current.text ?? '';
    if (current.type === 'hardBreak') return '\n';
    if (current.type === 'mention') return `@${mentionLabel(String(current.attrs?.userId))}`;
    if (current.type === 'inlineMath') return latexOf(current);
    if (current.type === 'blockMath') {
      lines.push(latexOf(current));
      return '';
    }
    const inner = (current.content ?? []).map(visit).join('');
    if (TEXT_BLOCKS.has(current.type)) lines.push(inner);
    return '';
  }
  visit(node);
  return lines.filter((line) => line !== '').join('\n');
}
