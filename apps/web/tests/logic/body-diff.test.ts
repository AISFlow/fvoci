/**
 * What changed from one body to another (body-diff.ts): blocks matched and
 * changed in place, words, letters of Korean, things that are not text, and
 * for any two bodies, that the merged one gives back both.
 */
import fc from 'fast-check';
import { describe, expect, test } from 'vitest';

import { diffBodies } from '$lib/logic/body-diff.ts';
import { foldSame } from '$lib/logic/diff-fold.ts';
import {
  blocksOf,
  canonical,
  DIFF_MARK,
  valueKey,
  type BodyNode,
  type DiffNode,
} from '$lib/logic/body-nodes.ts';

type Mark = { type: string; attrs?: Record<string, unknown> };

const text = (value: string, ...marks: (string | Mark)[]): BodyNode =>
  marks.length > 0
    ? {
        type: 'text',
        text: value,
        marks: marks.map((mark) => (typeof mark === 'string' ? { type: mark } : mark)),
      }
    : { type: 'text', text: value };
const p = (...content: (BodyNode | string)[]): BodyNode => ({
  type: 'paragraph',
  content: content.map((one) => (typeof one === 'string' ? text(one) : one)),
});
const h = (level: number, value: string): BodyNode => ({
  type: 'heading',
  attrs: { level },
  content: [text(value)],
});
const doc = (...content: BodyNode[]): BodyNode => ({ type: 'doc', content });
const li = (value: string): BodyNode => ({ type: 'listItem', content: [p(value)] });
const task = (checked: boolean, value: string): BodyNode => ({
  type: 'taskItem',
  attrs: { checked },
  content: [p(value)],
});
const code = (value: string, language?: string): BodyNode => ({
  type: 'codeBlock',
  ...(language ? { attrs: { language } } : {}),
  content: value === '' ? [] : [text(value)],
});
const image = (fileId: string, alt?: string): BodyNode => ({
  type: 'image',
  attrs: alt === undefined ? { fileId } : { fileId, alt },
});
const mention = (userId: string): BodyNode => ({ type: 'mention', attrs: { userId } });

/** What became of each piece of a merged block's inline content: `=text`, `+text`, `-text`, `~text`. */
function pieces(node: DiffNode | undefined): string[] {
  return (node?.content ?? []).map((child) => {
    const change = child.marks?.find((mark) => mark.type === DIFF_MARK)?.attrs?.change;
    const sign =
      change === 'added'
        ? '+'
        : change === 'removed'
          ? '-'
          : change === 'format'
            ? '~'
            : change === 'break'
              ? '/'
              : '=';
    return sign + (child.type === 'text' ? (child.text ?? '') : `<${child.type}>`);
  });
}

const withoutDiff = (marks: Mark[] | undefined): Mark[] | undefined => {
  const kept = (marks ?? []).filter((mark) => mark.type !== DIFF_MARK);
  return kept.length > 0 ? kept : undefined;
};

/** One side of a merged body back: the second (`after`), or the first without its marks (`before`). */
function side(blocks: DiffNode[], which: 'before' | 'after'): BodyNode {
  const dropped = which === 'after' ? 'removed' : 'added';
  const back = (node: DiffNode): BodyNode | undefined => {
    if (node.diff === dropped) return undefined;
    const change = node.marks?.find((mark) => mark.type === DIFF_MARK)?.attrs?.change;
    if (change === dropped || change === 'break') return undefined;
    const out: BodyNode = { type: which === 'before' && node.was ? node.was.type : node.type };
    const attrs = which === 'before' && node.was ? node.was.attrs : node.attrs;
    if (attrs) out.attrs = attrs;
    if (node.text !== undefined) out.text = node.text;
    const marks = which === 'before' ? undefined : withoutDiff(node.marks);
    if (marks) out.marks = marks;
    if (node.content)
      out.content = node.content.flatMap((child) => {
        const made = back(child);
        return made ? [made] : [];
      });
    return out;
  };
  return canonical(doc(...blocks.flatMap((block) => back(block) ?? [])));
}

function unmarked(node: BodyNode): BodyNode {
  const { marks: _marks, ...rest } = node;
  return canonical({ ...rest, ...(node.content ? { content: node.content.map(unmarked) } : {}) });
}

describe('bodies that say the same', () => {
  test('no body, an empty body and an empty paragraph are the same', () => {
    expect(diffBodies(null, doc(p())).changed).toBe(false);
    expect(diffBodies(doc(p()), null).blocks).toEqual([]);
  });

  test('a body against itself has no change, nor do texts cut and marks ordered otherwise', () => {
    const a = doc(p(text('a ', 'bold', 'italic'), text('b', 'bold', 'italic')));
    const b = doc(p(text('a b', 'italic', 'bold')));
    expect(diffBodies(a, a).changed).toBe(false);
    const same = diffBodies(a, b);
    expect(same.changed).toBe(false);
    expect(same.added + same.removed + same.formatted).toBe(0);
  });
});

describe('blocks', () => {
  test('from nothing everything is added, to nothing everything removed', () => {
    const body = doc(p('one two'), h(2, 'three'));
    const added = diffBodies(null, body);
    expect(added.blocks.map((block) => block.diff)).toEqual(['added', 'added']);
    expect(added.added).toBe(3);
    const removed = diffBodies(body, null);
    expect(removed.blocks.map((block) => block.diff)).toEqual(['removed', 'removed']);
    expect(removed.removed).toBe(3);
  });

  test('a block moved is removed where it was and added where it went', () => {
    const result = diffBodies(
      doc(p('alpha'), p('beta'), p('gamma'), p('moved here')),
      doc(p('moved here'), p('alpha'), p('beta'), p('gamma')),
    );
    expect(result.blocks.map((block) => block.diff)).toEqual([
      'added',
      undefined,
      undefined,
      undefined,
      'removed',
    ]);
  });

  test('a list keeps its items: the one edited is changed in place, word by word', () => {
    const result = diffBodies(
      doc({ type: 'bulletList', content: [li('milk'), li('bread and butter'), li('eggs')] }),
      doc({ type: 'bulletList', content: [li('milk'), li('bread and jam'), li('eggs')] }),
    );
    const [list] = result.blocks;
    expect(list?.diff).toBe('changed');
    expect(list?.content?.map((item) => item.diff)).toEqual([undefined, 'changed', undefined]);
    expect(pieces(list?.content?.[1]?.content?.[0])).toEqual(['=bread and ', '-butter', '+jam']);
    expect([result.added, result.removed]).toEqual([1, 1]);
  });

  test('a heading of another level, a task checked, a code block of another language say what they were', () => {
    const result = diffBodies(
      doc(h(2, 'Plan'), { type: 'taskList', content: [task(false, 'call')] }, code('x', 'js')),
      doc(h(1, 'Plan'), { type: 'taskList', content: [task(true, 'call')] }, code('x', 'ts')),
    );
    const [heading, list, block] = result.blocks;
    expect(heading?.was).toEqual({ type: 'heading', attrs: { level: 2 } });
    expect(list?.content?.[0]?.was).toEqual({ type: 'taskItem', attrs: { checked: false } });
    expect(block?.was).toEqual({ type: 'codeBlock', attrs: { language: 'js' } });
    expect(result.reshaped).toBe(3);
    expect(result.added + result.removed).toBe(0);
  });

  test('a paragraph removed, the next one touched and one added: each is what it is', () => {
    const result = diffBodies(
      doc(p('Apples are red and sweet'), p('Bananas grow in bunches')),
      doc(p('Bananas grow in big bunches'), p('Cherries are small')),
    );
    expect(result.blocks.map((block) => block.diff)).toEqual(['removed', 'changed', 'added']);
    expect(pieces(result.blocks[1])).toEqual(['=Bananas grow in ', '+big ', '=bunches']);
    expect([result.added, result.removed]).toEqual([4, 5]);
    // Rewritten where they stood, the two say nothing alike: each changed in its place.
    const rewritten = diffBodies(doc(p('one'), p('two')), doc(p('uno'), p('dos')));
    expect(rewritten.blocks.map((block) => block.diff)).toEqual(['changed', 'changed']);
  });

  test('a paragraph made a heading is the same block, of another kind', () => {
    const [block] = diffBodies(doc(p('Title')), doc(h(1, 'Title'))).blocks;
    expect(block).toMatchObject({ type: 'heading', diff: 'changed', was: { type: 'paragraph' } });
  });

  test('a code block is compared line by line', () => {
    const [block] = diffBodies(
      doc(code('a = 1\nb = 2\nc = 3')),
      doc(code('a = 1\nb = 20\nc = 3')),
    ).blocks;
    expect(pieces(block)).toEqual(['=a = 1\n', '-b = 2', '/\n', '+b = 20', '=\nc = 3']);
  });

  test('a line added after the last, which has no end, is one line added; the last removed, one removed', () => {
    const added = diffBodies(doc(code('a = 1\nb = 2')), doc(code('a = 1\nb = 2\nc = 3')));
    expect(pieces(added.blocks[0])).toEqual(['=a = 1\nb = 2', '+\nc = 3']);
    expect([added.added, added.removed]).toEqual([1, 0]);
    const removed = diffBodies(doc(code('a = 1\nb = 2')), doc(code('a = 1')));
    expect(pieces(removed.blocks[0])).toEqual(['=a = 1', '-\nb = 2']);
    expect([removed.added, removed.removed]).toEqual([0, 1]);
    // An empty line between two lines changed stays, the same in both.
    const around = diffBodies(doc(code('p\n\nq')), doc(code('r\n\ns')));
    expect(pieces(around.blocks[0])).toEqual(['-p', '/\n', '+r', '=\n\n', '-q', '/\n', '+s']);
    expect([around.added, around.removed]).toEqual([2, 2]);
    // An empty line added in place of a last line with no end is a line of its own.
    const emptied = diffBodies(doc(code('x')), doc(code('\ny')));
    expect(pieces(emptied.blocks[0])).toEqual(['-x', '/\n', '+\ny']);
    const rewritten = diffBodies(doc(code('a\nb\nc\nd')), doc(code('a\nx\ny\nd')));
    expect(pieces(rewritten.blocks[0])).toEqual(['=a\n', '-b\nc', '/\n', '+x\ny', '=\nd']);
    expect([rewritten.added, rewritten.removed]).toEqual([2, 2]);
  });

  test('a diagram is removed and added whole, its source meaning nothing line by line', () => {
    const result = diffBodies(
      doc(code('graph TD\nA-->B', 'mermaid')),
      doc(code('graph TD\nA-->C', 'mermaid')),
      { diagramLanguage: 'mermaid' },
    );
    expect(result.blocks.map((block) => block.diff)).toEqual(['removed', 'added']);
  });

  test('a picture of another file is removed and added; of the same file with another text, changed', () => {
    const swapped = diffBodies(doc(image('f1')), doc(image('f2')));
    expect(swapped.blocks.map((block) => block.diff)).toEqual(['removed', 'added']);
    expect([swapped.added, swapped.removed]).toEqual([1, 1]);
    const [described] = diffBodies(doc(image('f1', 'a cat')), doc(image('f1', 'two cats'))).blocks;
    expect(described).toMatchObject({ diff: 'changed', was: { attrs: { alt: 'a cat' } } });
  });

  test('a formula changed is the old one removed and the new one added', () => {
    const math = (latex: string): BodyNode => ({ type: 'blockMath', attrs: { latex } });
    const result = diffBodies(doc(p('see'), math('x^2')), doc(p('see'), math('x^3')));
    expect(result.blocks.map((block) => block.diff)).toEqual([undefined, 'removed', 'added']);
  });
});

describe('words', () => {
  test('a word replaced shows the old one removed and the new one added', () => {
    const [block] = diffBodies(doc(p('the quick brown fox')), doc(p('the slow brown fox'))).blocks;
    expect(pieces(block)).toEqual(['=the ', '-quick', '+slow', '= brown fox']);
  });

  test('a sentence rewritten reads as one removed and one added, not cut at its spaces', () => {
    const [block] = diffBodies(doc(p('we go home now')), doc(p('they run far now'))).blocks;
    expect(pieces(block)).toEqual(['-we go home', '+they run far', '= now']);
  });

  test('Korean: a particle or an ending changed shows the letters, an eojeol replaced shows the eojeol', () => {
    const [ending] = diffBodies(doc(p('밥을 먹었다')), doc(p('밥을 먹혔다'))).blocks;
    expect(pieces(ending)).toEqual(['=밥을 먹', '-었', '+혔', '=다']);
    const [particle] = diffBodies(doc(p('사과를 샀다')), doc(p('사과가 샀다'))).blocks;
    expect(pieces(particle)).toEqual(['=사과', '-를', '+가', '= 샀다']);
    const [replaced] = diffBodies(doc(p('판을 남겼다')), doc(p('글을 남겼다'))).blocks;
    expect(pieces(replaced)).toEqual(['-판을', '+글을', '= 남겼다']);
  });

  test('an emoji and a syllable are never cut', () => {
    const [block] = diffBodies(doc(p('좋아요 👍🏻')), doc(p('좋아요 👍🏽'))).blocks;
    const changed = pieces(block).filter((piece) => !piece.startsWith('='));
    expect(changed).toEqual(['-👍🏻', '+👍🏽']);
    // What is marked is counted, though it is no word: so is a sign, and an empty paragraph.
    const counted = (before: BodyNode, after: BodyNode): number[] => {
      const result = diffBodies(before, after);
      return [result.added, result.removed];
    };
    expect(counted(doc(p('좋아요 👍🏻')), doc(p('좋아요 👍🏽')))).toEqual([1, 1]);
    expect(counted(doc(p('끝났다.')), doc(p('끝났다!')))).toEqual([1, 1]);
    expect(counted(doc(p('a')), doc(p('a'), p()))).toEqual([1, 0]);
    const [family] = diffBodies(doc(p('a 👨‍👩‍👧 b')), doc(p('a 👨‍👩‍👦 b'))).blocks;
    expect(pieces(family)).toContain('-👨‍👩‍👧');
  });

  test('a letter composed and the same decomposed are other letters, each whole', () => {
    const [block] = diffBodies(doc(p('cafe\u0301 ok')), doc(p('caf\u00e9 ok'))).blocks;
    expect(pieces(block)).toEqual(['=caf', '-e\u0301', '+\u00e9', '= ok']);
  });

  test('marks alone changed are the same text, marked as formatted', () => {
    const result = diffBodies(doc(p('make it bold')), doc(p('make it ', text('bold', 'bold'))));
    expect(pieces(result.blocks[0])).toEqual(['=make it ', '~bold']);
    expect([result.added, result.removed, result.formatted]).toEqual([0, 0, 1]);
  });

  test('a mention of someone else is the one removed and the other added', () => {
    const [block] = diffBodies(
      doc(p('ask ', mention('u1'), ' now')),
      doc(p('ask ', mention('u2'), ' now')),
    ).blocks;
    expect(pieces(block)).toEqual(['=ask ', '-<mention>', '+<mention>', '= now']);
  });

  test('a very long paragraph edited in two places shows the two places', () => {
    const many = Array.from({ length: 50_000 }, (_, index) => `word${String(index)}`);
    const edited = [...many];
    edited[10] = 'changed';
    edited[49_990] = 'again';
    const started = performance.now();
    const result = diffBodies(doc(p(many.join(' '))), doc(p(edited.join(' '))));
    expect(performance.now() - started).toBeLessThan(5000);
    expect([result.added, result.removed]).toEqual([2, 2]);
    expect(result.coarse).toBe(false);
  });
});

describe('the budget', () => {
  test('a body compared beyond it is compared whole where it ran out, and still gives back both', () => {
    const a = doc(...Array.from({ length: 200 }, (_, index) => p(`a${String(index)} x`)));
    const b = doc(...Array.from({ length: 200 }, (_, index) => p(`b${String(index)} y`)));
    const result = diffBodies(a, b, { budget: 1000 });
    expect(result.coarse).toBe(true);
    expect(side(result.blocks, 'after')).toEqual(canonical(b));
    expect(side(result.blocks, 'before')).toEqual(unmarked(a));
  });
});

describe('folding what is the same', () => {
  const same = (value: string): DiffNode => p(value);
  const changed = (value: string): DiffNode => ({ ...p(value), diff: 'changed' });

  test('a run of the same between changes folds but for one block next to each change', () => {
    const blocks = [changed('a'), ...['1', '2', '3', '4', '5'].map(same), changed('b')];
    expect(foldSame(blocks).map((segment) => [segment.folded, segment.blocks.length])).toEqual([
      [false, 2],
      [true, 3],
      [false, 2],
    ]);
  });

  test('a short run stays, and a body with no change folds whole', () => {
    expect(foldSame([changed('a'), same('1'), same('2'), changed('b')])).toHaveLength(1);
    const segments = foldSame(['1', '2', '3', '4'].map(same));
    expect(segments).toEqual([{ folded: true, blocks: ['1', '2', '3', '4'].map(same) }]);
  });
});

/** Bodies of a few kinds of block, of words that repeat, some of them Korean and emoji. */
const WORDS = ['a', 'b', 'cat', 'dog', '사과를', '사과가', '먹었다', '👍🏻', '👍🏽', '.', ' '];
const inline = fc
  .array(
    fc.oneof(
      { weight: 5, arbitrary: fc.record({ word: fc.constantFrom(...WORDS), bold: fc.boolean() }) },
      { weight: 1, arbitrary: fc.constant({ word: '', bold: false }) },
    ),
    { maxLength: 8 },
  )
  .map((parts) =>
    parts.map(({ word, bold }) =>
      word === '' ? mention('u1') : bold ? text(`${word} `, 'bold') : text(`${word} `),
    ),
  );
const block: fc.Arbitrary<BodyNode> = fc.oneof(
  inline.map((content) => ({ type: 'paragraph', content })),
  inline.map((content) => ({ type: 'heading', attrs: { level: 2 }, content })),
  fc
    .array(fc.constantFrom('x = 1', 'y = 2', 'z'), { maxLength: 4 })
    .map((lines) => code(lines.join('\n'))),
  fc.array(inline, { minLength: 1, maxLength: 3 }).map((items) => ({
    type: 'bulletList',
    content: items.map((content) => ({
      type: 'listItem',
      content: [{ type: 'paragraph', content }],
    })),
  })),
  fc.constantFrom(image('f1'), image('f2', 'alt')),
);
const body = fc.array(block, { maxLength: 6 }).map((blocks) => doc(...blocks));

describe('any two bodies', () => {
  test('the merged body without what was removed is the second, without what was added the first', () => {
    fc.assert(
      fc.property(body, body, (a, b) => {
        const result = diffBodies(a, b);
        expect(side(result.blocks, 'after')).toEqual(canonical(doc(...blocksOf(b))));
        expect(side(result.blocks, 'before')).toEqual(unmarked(doc(...blocksOf(a))));
      }),
      { numRuns: 400 },
    );
  });

  test('a body against itself has no change', () => {
    fc.assert(
      fc.property(body, (a) => {
        expect(diffBodies(a, structuredClone(a)).changed).toBe(false);
      }),
    );
  });
});

describe('the key of a value', () => {
  test('is the same for equal values, the fields in any order, and differs for any other', () => {
    expect(valueKey({ a: 1, b: 'x' })).toBe(valueKey({ b: 'x', a: 1 }));
    const values: unknown[] = [
      1,
      '1',
      true,
      'true',
      null,
      undefined,
      [],
      {},
      [1, 2],
      ['1,2'],
      { a: 'x,b=s1:y' },
      { a: 'x', b: 'y' },
      { a: { b: 1 } },
      { 'a=': 1 },
    ];
    expect(new Set(values.map(valueKey)).size).toBe(values.length);
    fc.assert(
      fc.property(fc.jsonValue(), fc.jsonValue(), (a, b) => {
        expect(valueKey(a) === valueKey(b)).toBe(sameJson(a, b));
      }),
    );
  });
});

/** Whether two JSON values are equal, the fields of objects in any order. */
function sameJson(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const names = Object.keys(left);
  return (
    names.length === Object.keys(right).length &&
    names.every((name) => Object.hasOwn(right, name) && sameJson(left[name], right[name]))
  );
}

describe('what is counted', () => {
  const lineBreak: BodyNode = { type: 'hardBreak' };

  test('a list numbered from elsewhere is reshaped, and says from what number to what', () => {
    const list = (start?: number): BodyNode => ({
      type: 'orderedList',
      ...(start === undefined ? {} : { attrs: { start } }),
      content: [{ type: 'listItem', content: [p('one')] }],
    });
    const result = diffBodies(doc(list()), doc(list(5)));
    expect(result.reshaped).toBe(1);
    // From 1, which is not stored: the list's own start is 1 when it has none.
    expect(result.blocks[0]?.was).toEqual({ type: 'orderedList' });
    expect(result.blocks[0]?.attrs).toEqual({ start: 5 });
  });

  test('a line break counts the same in a paragraph changed and in one removed whole: as nothing', () => {
    const changed = diffBodies(doc(p('one')), doc(p('one', lineBreak, 'two')));
    const whole = diffBodies(doc(p('zero')), doc(p('zero'), p('one', lineBreak, 'two')));
    expect(changed.added).toBe(1);
    expect(whole.added).toBe(2);
    // A break alone added is a change, counted one.
    expect(diffBodies(doc(p('one two')), doc(p('one', lineBreak, 'two'))).added).toBe(1);
  });

  test('a diagram added or removed whole counts one, as a picture does, not the words of its source', () => {
    const diagram = code('graph TD\nA-->B\nB-->C', 'mermaid');
    const options = { diagramLanguage: 'mermaid' };
    const added = diffBodies(doc(p('see')), doc(p('see'), diagram), options);
    const removed = diffBodies(doc(p('see'), diagram), doc(p('see')), options);
    expect([added.added, removed.removed]).toEqual([1, 1]);
  });
});
