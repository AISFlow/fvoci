/** A card's preview is made as the server makes it (`CardPreview` in the contract). */
import { describe, expect, it } from 'vitest';

import type { DocNode } from '$lib/editor/schema.ts';
import { cardPreviewOf, PREVIEW_TEXT_LENGTH } from '$lib/editor/card-preview.ts';

const paragraph = (...content: DocNode[]): DocNode => ({ type: 'paragraph', content });
const words = (text: string): DocNode => ({ type: 'text', text });
const image = (fileId: string): DocNode => ({ type: 'image', attrs: { fileId } });
const doc = (...content: DocNode[]): DocNode => ({ type: 'doc', content });

describe('a card preview', () => {
  it('is empty for no body', () => {
    expect(cardPreviewOf(null)).toEqual({ imageFileId: '', text: '' });
  });

  it('takes the first image in document order, also inside a list', () => {
    const body = doc(
      paragraph(words('Before')),
      {
        type: 'bulletList',
        content: [{ type: 'listItem', content: [image('first')] }],
      },
      image('second'),
    );
    expect(cardPreviewOf(body).imageFileId).toBe('first');
    const table = doc({
      type: 'table',
      content: [
        {
          type: 'tableRow',
          content: [{ type: 'tableCell', content: [paragraph(words('x')), image('cell')] }],
        },
      ],
    });
    expect(cardPreviewOf(table).imageFileId).toBe('cell');
  });

  it('gives a line per block, nothing for a mention, a formula its source', () => {
    const body = doc(
      { type: 'heading', attrs: { level: 1 }, content: [words('Plan')] },
      paragraph(words('Ask '), { type: 'mention', attrs: { userId: 'u1' } }, words(' about '), {
        type: 'inlineMath',
        attrs: { latex: 'x^2' },
      }),
      paragraph(),
      { type: 'blockMath', attrs: { latex: 'E=mc^2' } },
    );
    expect(cardPreviewOf(body).text).toBe('Plan\nAsk  about x^2\nE=mc^2');
  });

  it('is cut at 200 UTF-16 code units between characters, without trailing white space', () => {
    expect(cardPreviewOf(doc(paragraph(words('a'.repeat(200))))).text).toHaveLength(200);
    expect(cardPreviewOf(doc(paragraph(words('a'.repeat(201))))).text).toHaveLength(200);
    // An emoji of two units does not lose its half; a flag of four stays whole or goes.
    const emoji = cardPreviewOf(doc(paragraph(words(`${'a'.repeat(199)}😀`)))).text;
    expect(emoji).toBe('a'.repeat(199));
    const flag = cardPreviewOf(doc(paragraph(words(`${'a'.repeat(197)}🇰🇷`)))).text;
    expect(flag).toBe('a'.repeat(197));
    // A letter with its combining accent stays whole.
    const accent = cardPreviewOf(doc(paragraph(words(`${'a'.repeat(199)}é`)))).text;
    expect(accent).toBe('a'.repeat(199));
    // A cut that ends in spaces loses them.
    const spaced = cardPreviewOf(doc(paragraph(words(`${'a'.repeat(198)}   b`))));
    expect(spaced.text).toBe('a'.repeat(198));
    expect(spaced.text.length).toBeLessThanOrEqual(PREVIEW_TEXT_LENGTH);
    expect(cardPreviewOf(doc(paragraph(words('Done.   '))))).toEqual({
      imageFileId: '',
      text: 'Done.',
    });
  });
});
