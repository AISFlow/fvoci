/**
 * The comparison of the largest bodies there can be (editor/schema.ts:
 * 8 MiB, 100,000 nodes): edited in places it finds every place, and wholly
 * other it answers coarsely within its budget (`DIFF_BUDGET`) rather than
 * holding the page. The times are written out for the record; what is
 * asserted is what was found, not how fast, which depends on the machine.
 */
import { describe, expect, test } from 'vitest';

import { MAX_BODY_BYTES, MAX_NODES } from '$lib/editor/schema.ts';
import { diffBodies } from '$lib/logic/body-diff.ts';
import type { BodyNode } from '$lib/logic/body-nodes.ts';

/** Words that repeat as in a text, from a small seeded generator. */
function wordsFrom(seed: number): () => string {
  const vocabulary = [
    '판을',
    '남겼다',
    '본문',
    'the',
    'version',
    'kept',
    '사과를',
    'body',
    '고쳤다',
    'and',
  ];
  let state = seed;
  return () => {
    state = (state * 1_103_515_245 + 12_345) % 2_147_483_648;
    return vocabulary[state % vocabulary.length] ?? 'x';
  };
}

const paragraph = (text: string): BodyNode => ({
  type: 'paragraph',
  content: [{ type: 'text', text }],
});
const doc = (content: BodyNode[]): BodyNode => ({ type: 'doc', content });
const bytes = (body: BodyNode): number => new TextEncoder().encode(JSON.stringify(body)).length;

function timed<T>(name: string, run: () => T): T {
  const started = performance.now();
  const result = run();
  console.warn(`${name}: ${(performance.now() - started).toFixed(0)} ms`);
  return result;
}

// Alone, each takes under a second; beside the rest of the tests, several times as long.
describe('the largest bodies', { timeout: 30_000 }, () => {
  test('8 MiB in paragraphs, one word in every hundredth paragraph changed', () => {
    const next = wordsFrom(1);
    const texts: string[] = [];
    while (bytes(doc(texts.map(paragraph))) < MAX_BODY_BYTES - 300 * 200) {
      for (let index = 0; index < 200; index += 1)
        texts.push(Array.from({ length: 30 }, next).join(' '));
    }
    const before = doc(texts.map(paragraph));
    const after = doc(
      texts.map((text, index) => paragraph(index % 100 === 0 ? `${text} edited` : text)),
    );
    expect(bytes(after)).toBeLessThanOrEqual(MAX_BODY_BYTES);
    const result = timed(`8 MiB, ${String(texts.length)} paragraphs, 1% edited`, () =>
      diffBodies(before, after),
    );
    expect(result.added).toBe(Math.ceil(texts.length / 100));
    expect(result.removed).toBe(0);
    expect(result.coarse).toBe(false);
  });

  test('8 MiB in paragraphs, every one other', () => {
    const one = wordsFrom(2);
    const other = wordsFrom(3);
    const count = 24_000;
    const before = doc(
      Array.from({ length: count }, () => paragraph(Array.from({ length: 30 }, one).join(' '))),
    );
    const after = doc(
      Array.from({ length: count }, () => paragraph(Array.from({ length: 30 }, other).join(' '))),
    );
    expect(bytes(before)).toBeLessThanOrEqual(MAX_BODY_BYTES);
    const result = timed(`8 MiB, ${String(count)} paragraphs, all other`, () =>
      diffBodies(before, after),
    );
    expect(result.changed).toBe(true);
    expect(result.blocks.length).toBeGreaterThanOrEqual(count);
  });

  test('one paragraph of 8 MiB, edited in fifty places', () => {
    const next = wordsFrom(4);
    const words: string[] = [];
    let length = 0;
    while (length < MAX_BODY_BYTES / 3) {
      const word = next();
      words.push(word);
      length += new TextEncoder().encode(word).length + 1;
    }
    const edited = [...words];
    for (let place = 0; place < 50; place += 1)
      edited[Math.floor((place * words.length) / 50)] = 'EDITED';
    const before = doc([paragraph(words.join(' '))]);
    const after = doc([paragraph(edited.join(' '))]);
    expect(bytes(after)).toBeLessThanOrEqual(MAX_BODY_BYTES);
    const result = timed(`one paragraph of ${String(words.length)} words, 50 edits`, () =>
      diffBodies(before, after),
    );
    expect(result.added).toBe(50);
    expect(result.coarse).toBe(false);
  });

  test('100,000 nodes, a word in one paragraph of a hundred changed, then every one other', () => {
    const count = MAX_NODES / 2;
    const before = doc(
      Array.from({ length: count }, (_, index) => paragraph(`line ${String(index)}`)),
    );
    const after = doc(
      Array.from({ length: count }, (_, index) =>
        paragraph(index % 100 === 0 ? `row ${String(index)}` : `line ${String(index)}`),
      ),
    );
    const result = timed(`${String(MAX_NODES)} nodes, 1% edited`, () => diffBodies(before, after));
    expect(result.added).toBe(count / 100);
    expect(result.coarse).toBe(false);
    const other = doc(
      Array.from({ length: count }, (_, index) => paragraph(`row ${String(index + count)}`)),
    );
    const all = timed(`${String(MAX_NODES)} nodes, all other`, () => diffBodies(before, other));
    expect(all.changed).toBe(true);
  });
});
