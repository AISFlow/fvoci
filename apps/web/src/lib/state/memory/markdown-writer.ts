/**
 * A body in Markdown in the in-memory backend, near enough to stand in for
 * the server's writer (`crates/server/src/app/body_markdown/write.rs`): text
 * and its marks, links, headings, lists and their tasks, quotes, code and
 * rules as Markdown; a reference as `[[id]]`, a person by `@`, and of what
 * else a body holds (a table, a file) the text in it.
 */
import type { DocMark, DocNode } from '$lib/editor/schema.ts';

/** The attribute `name` of a node or a mark as text; empty for none. */
function attr(of: DocNode | DocMark, name: string): string {
  const value = of.attrs?.[name];
  return typeof value === 'string' || typeof value === 'number' ? String(value) : '';
}

/** `text` with what Markdown would read as more than text escaped. */
function escaped(text: string): string {
  return text.replace(/[\\`*_[\]<>#~|]/g, (character) => `\\${character}`);
}

/** `text` inside `mark`. */
function marked(text: string, mark: DocMark): string {
  switch (mark.type) {
    case 'bold':
      return `**${text}**`;
    case 'italic':
      return `*${text}*`;
    case 'strike':
      return `~~${text}~~`;
    case 'link':
      return `[${text}](<${attr(mark, 'href')}>)`;
    default:
      return text;
  }
}

/** What a node of a line says. */
function inline(node: DocNode): string {
  switch (node.type) {
    case 'text': {
      const marks = node.marks ?? [];
      const text = node.text ?? '';
      if (marks.some((mark) => mark.type === 'code')) return `\`${text.replace(/`/g, "'")}\``;
      return marks.reduce(marked, escaped(text));
    }
    case 'hardBreak':
      return '\\\n';
    case 'reference':
      return `[[${attr(node, 'id')}]]`;
    case 'mention':
      return `@${attr(node, 'userId')}`;
    default:
      return (node.content ?? []).map(inline).join('');
  }
}

/** `text` with `prefix` before its first line and `rest` before the others. */
function indented(text: string, prefix: string, rest: string): string {
  return text
    .split('\n')
    .map((line, index) => (index === 0 ? prefix : line === '' ? '' : rest) + line)
    .join('\n');
}

/** The items of a list, each after its marker. */
function items(list: DocNode, marker: (index: number, item: DocNode) => string): string {
  return (list.content ?? [])
    .map((item, index) => {
      const lead = marker(index, item);
      return indented(blocks(item.content ?? []), lead, ' '.repeat(lead.length));
    })
    .join('\n');
}

/** What a block says. */
function block(node: DocNode): string {
  switch (node.type) {
    case 'paragraph':
      return inline(node);
    case 'heading': {
      const level = Math.min(Math.max(Number(node.attrs?.level ?? 1), 1), 3);
      return `${'#'.repeat(level)} ${inline(node)}`;
    }
    case 'bulletList':
      return items(node, () => '- ');
    case 'orderedList':
      return items(node, (index) => `${String(index + 1)}. `);
    case 'taskList':
      return items(node, (_, item) => (item.attrs?.checked === true ? '- [x] ' : '- [ ] '));
    case 'blockquote':
      return indented(blocks(node.content ?? []), '> ', '> ');
    case 'codeBlock': {
      const language = attr(node, 'language');
      const code = (node.content ?? []).map((text) => text.text ?? '').join('');
      return `\`\`\`${language}\n${code}\n\`\`\``;
    }
    case 'horizontalRule':
      return '---';
    default:
      return node.content?.some((child) => child.type === 'text')
        ? inline(node)
        : blocks(node.content ?? []);
  }
}

/** Blocks one after the other, an empty line between them. */
function blocks(nodes: readonly DocNode[]): string {
  return nodes
    .map(block)
    .filter((text) => text !== '')
    .join('\n\n');
}

/** The body in Markdown; empty for none. */
export function markdownOf(body: DocNode | null | undefined): string {
  return body ? blocks(body.content ?? []) : '';
}
