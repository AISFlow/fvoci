/** What is typed into the comment field becomes a document: a line is a paragraph. */
import { describe, expect, it } from 'vitest';

import { validateDoc } from '$lib/editor/schema.ts';
import { commentDoc, commentText } from '$lib/features/card/comment-doc.ts';

const paragraph = (text: string): object => ({
  type: 'paragraph',
  content: [{ type: 'text', text }],
});

describe('the document of a comment', () => {
  it('is one paragraph for one line', () => {
    expect(commentDoc('Looks good')).toEqual({ type: 'doc', content: [paragraph('Looks good')] });
  });

  it('has a paragraph for every line', () => {
    expect(commentDoc('one\ntwo\nthree')?.content).toEqual([
      paragraph('one'),
      paragraph('two'),
      paragraph('three'),
    ]);
  });

  it('does not keep empty lines: they only set paragraphs apart', () => {
    expect(commentDoc('one\n\n\ntwo\n   \nthree')?.content).toEqual([
      paragraph('one'),
      paragraph('two'),
      paragraph('three'),
    ]);
  });

  it('reads the line ends of every system', () => {
    expect(commentDoc('one\r\ntwo\rthree')?.content).toEqual([
      paragraph('one'),
      paragraph('two'),
      paragraph('three'),
    ]);
  });

  it('drops the space at the end of a line and keeps the one at its start', () => {
    expect(commentDoc('  indented  \nplain\t')?.content).toEqual([
      paragraph('  indented'),
      paragraph('plain'),
    ]);
  });

  it('is nothing for text that is empty or all white space', () => {
    expect(commentDoc('')).toBeNull();
    expect(commentDoc('  \n\t\n')).toBeNull();
  });

  it('keeps Hangul, emoji and marks of other writing as they were typed', () => {
    const text = '다음 주 화요일까지 올릴게요 🙌';
    expect(commentDoc(text)?.content).toEqual([paragraph(text)]);
  });

  it('takes what looks like markup as text', () => {
    expect(commentDoc('<b>bold</b> **not** [a](b)')?.content).toEqual([
      paragraph('<b>bold</b> **not** [a](b)'),
    ]);
  });

  it('is a body the server accepts', () => {
    expect(validateDoc(commentDoc('one\n\ntwo'))).toEqual([]);
  });
});

describe('the text a stored comment is edited as', () => {
  it('gives a line per paragraph, and the same comment again when stored', () => {
    const doc = commentDoc('first line\n  indented\nlast');
    expect(commentText(doc)).toBe('first line\n  indented\nlast');
    expect(commentDoc(commentText(doc) ?? '')).toEqual(doc);
  });

  it('gives nothing for a comment that is more than plain paragraphs', () => {
    expect(commentText(null)).toBeUndefined();
    expect(
      commentText({
        type: 'doc',
        content: [
          { type: 'paragraph', content: [{ type: 'text', text: 'b', marks: [{ type: 'bold' }] }] },
        ],
      }),
    ).toBeUndefined();
    expect(
      commentText({ type: 'doc', content: [{ type: 'heading', attrs: { level: 1 } }] }),
    ).toBeUndefined();
  });
});
