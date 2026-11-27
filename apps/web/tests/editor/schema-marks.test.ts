import { expect, it } from 'vitest';

import { validateDoc } from '$lib/editor/schema.ts';

const withText = (text: unknown): unknown => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [text] }],
});
const at = '/doc[0]/paragraph[0]/text';

it('reports mark problems in stored order and duplicates after the marks', () => {
  expect(
    validateDoc(
      withText({
        type: 'text',
        text: 'x',
        marks: [
          null,
          { type: 'link', attrs: { href: 'javascript:x' }, extra: true },
          { type: 'link' },
        ],
      }),
    ),
  ).toEqual([
    `${at}: a mark is not an object`,
    `${at}@link: attribute "href" has an invalid value`,
    `${at}@link: unknown member "extra"`,
    `${at}@link: attribute "href" is missing`,
    `${at}: a mark is given twice`,
  ]);
});

it('stops before content validation when the marks member is not a list', () => {
  expect(validateDoc(withText({ type: 'text', text: '', marks: null, content: null }))).toEqual([
    `${at}: text is empty`,
    `${at}: marks is not a list`,
  ]);
});

it('returns every problem from a large marks list without throwing', () => {
  const marks = Array.from({ length: 150_000 }, () => null);
  const problems = validateDoc(withText({ type: 'text', text: 'x', marks }));
  expect(problems).toHaveLength(marks.length);
  expect(problems.every((problem) => problem === `${at}: a mark is not an object`)).toBe(true);
});
