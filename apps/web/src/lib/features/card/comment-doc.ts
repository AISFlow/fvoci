/**
 * The document a comment is stored as, from what was typed into the plain
 * comment field.
 *
 * The field is a text box, not the editor: a line of it becomes a paragraph.
 * Empty lines only set paragraphs apart and are not kept, and the white
 * space at the end of a line is dropped (the space at its start is kept: it
 * may be meant as an indent). Text that is all white space is no comment.
 */
import type { DocNode } from '$lib/editor/schema.ts';

/** The stored form of a comment; null when there is nothing to say. */
export function commentDoc(text: string): DocNode | null {
  const paragraphs = text
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .map((line) => line.trimEnd())
    .filter((line) => line.trim() !== '')
    .map((line): DocNode => ({ type: 'paragraph', content: [{ type: 'text', text: line }] }));
  return paragraphs.length === 0 ? null : { type: 'doc', content: paragraphs };
}

/**
 * The text to edit a stored comment as, back in the plain comment field: a
 * line per paragraph. Undefined for a comment that is more than paragraphs
 * of plain text (it came from elsewhere): the field would lose the rest.
 */
export function commentText(doc: DocNode | null): string | undefined {
  if (!doc) return undefined;
  const lines: string[] = [];
  for (const block of doc.content ?? []) {
    if (block.type !== 'paragraph') return undefined;
    let line = '';
    for (const inline of block.content ?? []) {
      if (inline.type !== 'text' || inline.marks?.length) return undefined;
      line += inline.text ?? '';
    }
    lines.push(line);
  }
  return lines.join('\n');
}
