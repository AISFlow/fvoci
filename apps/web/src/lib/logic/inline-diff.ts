/**
 * What changed in the inline content of one block, two bodies having it
 * (body-diff.ts): the block's content as it is in the second body, with the
 * first's text that is gone in it, each piece marked by what became of it
 * (`DIFF_MARK`: added, removed, or the same text with other marks).
 *
 * A text block is compared word by word (`Intl.Segmenter`'s words: a word of
 * Korean is an eojeol, the spaces between words and each mark of punctuation
 * a piece of their own); a code block line by line. A space alone between two
 * changes goes with them, so that a sentence rewritten reads as one removed
 * and one added, and not as words cut by unchanged spaces. A word that was
 * replaced by one that shares most of its letters with it (a particle or an
 * ending of Korean, a letter of a typo) shows only the letters that changed:
 * 먹[었→혔]다. Letters are grapheme clusters: an emoji or a syllable is never
 * cut.
 *
 * What both texts start and end with is taken off before the words are
 * found, at a space, so that a long paragraph edited in one place costs
 * little. A stretch that differs and is longer than `INLINE_LIMIT` is not
 * cut into words: it is said to be removed and added whole.
 */
import {
  countsOne,
  DIFF_MARK,
  marksKey,
  valueKey,
  wordCount,
  type BodyMark,
  type BodyNode,
  type InlineChange,
} from './body-nodes.ts';
import { diffSequence, type DiffBudget } from './sequence-diff.ts';

/** Past this many letters of the two sides of a stretch that differs, it is not cut into words. */
export const INLINE_LIMIT = 200_000;

/** The letter that stands for an inline thing that is not text (a mention, a formula, a break). */
const ATOM = '￼';

const words = new Intl.Segmenter(undefined, { granularity: 'word' });
const letters = new Intl.Segmenter(undefined, { granularity: 'grapheme' });

interface Inline {
  text: string;
  /** The inline nodes, each with where it starts and ends in `text`. */
  runs: { from: number; to: number; node: BodyNode }[];
  /** The key of each thing that is not text, by where it is in `text`. */
  atoms: Map<number, string>;
  /** Where the things that count as a word are: not the line breaks. */
  counted: Set<number>;
}

function inlineOf(block: BodyNode): Inline {
  let text = '';
  const runs: Inline['runs'] = [];
  const atoms = new Map<number, string>();
  const counted = new Set<number>();
  for (const node of block.content ?? []) {
    const from = text.length;
    if (node.type === 'text') text += node.text ?? '';
    else {
      text += ATOM;
      atoms.set(from, node.type + valueKey(node.attrs ?? {}));
      if (countsOne(node.type)) counted.add(from);
    }
    runs.push({ from, to: text.length, node });
  }
  return { text, runs, atoms, counted };
}

const sameAt = (a: Inline, i: number, b: Inline, j: number): boolean =>
  a.text[i] === b.text[j] && (a.text[i] !== ATOM || a.atoms.get(i) === b.atoms.get(j));

const SPACE = /\s/;
/** Where a text may be cut without cutting a word: at either end, or next to a space. */
const atBreak = (text: string, at: number): boolean =>
  at === 0 || at === text.length || SPACE.test(text[at - 1] ?? '') || SPACE.test(text[at] ?? '');

interface Token {
  from: number;
  to: number;
  key: string;
  space: boolean;
}

/** The pieces a text is compared in: lines (of code), runs between spaces, or words (`Intl.Segmenter`'s). */
type Unit = 'lines' | 'chunks' | 'words';

/** The key of a piece of text: the text, each thing in it that is not text by its own key. */
function keyOf(inline: Inline, from: number, segment: string): string {
  if (!segment.includes(ATOM)) return segment;
  let key = '';
  for (let at = 0; at < segment.length; at += 1) {
    const atom = segment[at] === ATOM ? inline.atoms.get(from + at) : undefined;
    key += atom === undefined ? (segment[at] ?? '') : `\u0000${atom}\u0000`;
  }
  return key;
}

const CHUNKS = /\s+|\uFFFC|[^\s\uFFFC]+/g;

function tokensOf(inline: Inline, from: number, to: number, unit: Unit): Token[] {
  const part = inline.text.slice(from, to);
  const tokens: Token[] = [];
  if (unit === 'chunks') {
    for (const match of part.matchAll(CHUNKS)) {
      const start = from + match.index;
      const segment = match[0];
      tokens.push({
        from: start,
        to: start + segment.length,
        key: keyOf(inline, start, segment),
        space: SPACE.test(segment[0] ?? ''),
      });
    }
    return tokens;
  }
  if (unit === 'lines') {
    // A line and its end are two: the last line, which has no end, is the same line given one.
    // The end of a line with text is as a space between two changes; an empty line is a line.
    let at = 0;
    while (at < part.length) {
      const end = part.indexOf('\n', at);
      const stop = end === -1 ? part.length : end;
      if (stop > at)
        tokens.push({ from: from + at, to: from + stop, key: part.slice(at, stop), space: false });
      if (end !== -1)
        tokens.push({ from: from + end, to: from + end + 1, key: '\n', space: stop > at });
      at = stop + 1;
    }
    return tokens;
  }
  for (const { segment, index } of words.segment(part)) {
    const start = from + index;
    tokens.push({
      from: start,
      to: start + segment.length,
      key: keyOf(inline, start, segment),
      space: /^\s+$/.test(segment),
    });
  }
  return tokens;
}

/** What the merged content is made of, before it is made: ranges of the first text or the second. */
type Piece =
  | { op: 'same'; aFrom: number; bFrom: number; length: number }
  | { op: 'removed' | 'added'; from: number; to: number };

/** A stretch that differs: what of the first text went, what of the second came. */
interface Hunk {
  aTokens: Token[];
  bTokens: Token[];
}

/** The hunks and kept stretches of two middles, the spaces alone between two changes going with them. */
function compareTokens(a: Token[], b: Token[], budget: DiffBudget): (Hunk | Piece)[] {
  const runs = diffSequence(
    a.map((token) => token.key),
    b.map((token) => token.key),
    budget,
  );
  const out: (Hunk | Piece)[] = [];
  let hunk: Hunk | undefined;
  const open = (): Hunk => {
    hunk ??= { aTokens: [], bTokens: [] };
    return hunk;
  };
  const take = (side: 'a' | 'b', tokens: Token[]): void => {
    if (tokens.length === 0) return;
    const into = open();
    (side === 'a' ? into.aTokens : into.bTokens).push(...tokens);
  };
  runs.forEach((run, index) => {
    const aPart = a.slice(run.from, run.from + (run.step === 'added' ? 0 : run.length));
    const bPart = b.slice(run.to, run.to + (run.step === 'removed' ? 0 : run.length));
    const between = index > 0 && index < runs.length - 1;
    if (run.step === 'same' && !(between && aPart.every((token) => token.space))) {
      if (hunk) out.push(hunk);
      hunk = undefined;
      const first = aPart[0];
      const firstB = bPart[0];
      const last = aPart.at(-1);
      if (first && firstB && last)
        out.push({
          op: 'same',
          aFrom: first.from,
          bFrom: firstB.from,
          length: last.to - first.from,
        });
      return;
    }
    take('a', aPart);
    take('b', bPart);
  });
  if (hunk) out.push(hunk);
  return out;
}

/** Common letters (grapheme clusters) at the start and at the end of two words, as lengths in letters. */
function sharedEnds(a: string, b: string): { prefix: number; suffix: number } | undefined {
  const ga = Array.from(letters.segment(a), (one) => one.segment);
  const gb = Array.from(letters.segment(b), (one) => one.segment);
  let first = 0;
  while (first < ga.length && first < gb.length && ga[first] === gb[first]) first += 1;
  let last = 0;
  while (
    last < ga.length - first &&
    last < gb.length - first &&
    ga[ga.length - 1 - last] === gb[gb.length - 1 - last]
  )
    last += 1;
  const shared = first + last;
  if (shared < 2 || shared * 2 < Math.max(ga.length, gb.length)) return undefined;
  return {
    prefix: ga.slice(0, first).join('').length,
    suffix: last === 0 ? 0 : ga.slice(ga.length - last).join('').length,
  };
}

/** Where a hunk is in each text: an empty range for a side that has none of it. */
function spanOf(hunk: Hunk): { aFrom: number; aTo: number; bFrom: number; bTo: number } {
  const aFrom = hunk.aTokens[0]?.from ?? 0;
  const bFrom = hunk.bTokens[0]?.from ?? 0;
  return {
    aFrom,
    aTo: hunk.aTokens.at(-1)?.to ?? aFrom,
    bFrom,
    bTo: hunk.bTokens.at(-1)?.to ?? bFrom,
  };
}

/**
 * The pieces of a hunk: what went, then what came; with `letters`, of one
 * word for one, only the letters that changed.
 */
function piecesOf(hunk: Hunk, byLetters: boolean): Piece[] {
  const { aFrom, aTo, bFrom, bTo } = spanOf(hunk);
  const [oneA] = hunk.aTokens;
  const [oneB] = hunk.bTokens;
  const single =
    byLetters &&
    hunk.aTokens.length === 1 &&
    hunk.bTokens.length === 1 &&
    oneA &&
    oneB &&
    !oneA.space &&
    !oneB.space &&
    !oneA.key.includes('\u0000') &&
    !oneB.key.includes('\u0000');
  const ends = single ? sharedEnds(oneA.key, oneB.key) : undefined;
  if (ends) {
    const pieces: Piece[] = [];
    if (ends.prefix > 0) pieces.push({ op: 'same', aFrom, bFrom, length: ends.prefix });
    pieces.push({ op: 'removed', from: aFrom + ends.prefix, to: aTo - ends.suffix });
    pieces.push({ op: 'added', from: bFrom + ends.prefix, to: bTo - ends.suffix });
    if (ends.suffix > 0)
      pieces.push({
        op: 'same',
        aFrom: aTo - ends.suffix,
        bFrom: bTo - ends.suffix,
        length: ends.suffix,
      });
    return pieces.filter((piece) => piece.op === 'same' || piece.to > piece.from);
  }
  const pieces: Piece[] = [];
  if (aTo > aFrom) pieces.push({ op: 'removed', from: aFrom, to: aTo });
  if (bTo > bFrom) pieces.push({ op: 'added', from: bFrom, to: bTo });
  return pieces;
}

const diffMark = (change: InlineChange): BodyMark => ({ type: DIFF_MARK, attrs: { change } });

/** The index of the run that `at` is in (the first that ends after it), by halves. */
function runAt(inline: Inline, at: number): number {
  let low = 0;
  let high = inline.runs.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if ((inline.runs[middle]?.to ?? 0) > at) high = middle;
    else low = middle + 1;
  }
  return low;
}

/** The inline nodes of `inline` from `from` to `to`, marked with `change` when there is one. */
function slice(inline: Inline, from: number, to: number, change?: InlineChange): BodyNode[] {
  const out: BodyNode[] = [];
  for (let index = runAt(inline, from); index < inline.runs.length; index += 1) {
    const run = inline.runs[index];
    if (!run || run.from >= to) break;
    const marks = [...(run.node.marks ?? []), ...(change ? [diffMark(change)] : [])];
    if (run.node.type !== 'text') {
      out.push(marks.length > 0 ? { ...run.node, marks } : run.node);
      continue;
    }
    const text = (run.node.text ?? '').slice(
      Math.max(from, run.from) - run.from,
      Math.min(to, run.to) - run.from,
    );
    out.push(marks.length > 0 ? { type: 'text', text, marks } : { type: 'text', text });
  }
  return out;
}

/** What changed in the inline content of a block: the merged content, and how much. */
export interface InlineDiff {
  content: BodyNode[];
  /** Whether any of it is marked: a change of spaces alone counts no word, and is one. */
  changed: boolean;
  added: number;
  removed: number;
  formatted: number;
}

/**
 * How much a piece of text counts: its words, or its lines; a thing that is
 * not text counts one, as in a block removed whole (`weightOf`). The end of a line at either edge of the piece belongs
 * to the line next to it.
 */
function weight(inline: Inline, from: number, to: number, unit: 'words' | 'lines'): number {
  const text = inline.text.slice(from, to);
  if (unit === 'words') {
    let things = 0;
    for (let at = text.indexOf(ATOM); at !== -1; at = text.indexOf(ATOM, at + 1))
      if (inline.counted.has(from + at)) things += 1;
    return wordCount(text.replaceAll(ATOM, ' ')) + things;
  }
  const lines = text.split('\n');
  if (lines[0] === '') lines.shift();
  if (lines.at(-1) === '') lines.pop();
  return lines.length;
}

class Merging {
  readonly content: BodyNode[] = [];
  changed = false;
  added = 0;
  removed = 0;
  formatted = 0;

  readonly a: Inline;
  readonly b: Inline;
  readonly unit: 'words' | 'lines';

  constructor(a: Inline, b: Inline, unit: 'words' | 'lines') {
    this.a = a;
    this.b = b;
    this.unit = unit;
  }

  /** Puts nodes after the content, a text joined to the one before when their marks are the same. */
  #push(nodes: BodyNode[]): void {
    for (const node of nodes) {
      const last = this.content.at(-1);
      if (
        last?.type === 'text' &&
        node.type === 'text' &&
        marksKey(last.marks) === marksKey(node.marks)
      )
        last.text = (last.text ?? '') + (node.text ?? '');
      else this.content.push(node);
    }
  }

  /** A stretch the same in both: the second's, marked where its marks are others than the first's. */
  same(aFrom: number, bFrom: number, length: number): void {
    const { a, b } = this;
    let i = aFrom;
    let j = bFrom;
    let ra = runAt(a, i);
    let rb = runAt(b, j);
    while (i < aFrom + length) {
      const runA = a.runs[ra];
      const runB = b.runs[rb];
      if (!runA || !runB) break;
      const step = Math.min(runA.to - i, runB.to - j, aFrom + length - i);
      const other = marksKey(runA.node.marks) !== marksKey(runB.node.marks);
      if (other) {
        this.changed = true;
        this.formatted += Math.max(1, weight(b, j, j + step, 'words'));
      }
      this.#push(slice(b, j, j + step, other ? 'format' : undefined));
      i += step;
      j += step;
      if (i >= runA.to) ra += 1;
      if (j >= runB.to) rb += 1;
    }
  }

  put(piece: Piece): void {
    if (piece.op === 'same') {
      this.same(piece.aFrom, piece.bFrom, piece.length);
      return;
    }
    this.changed = true;
    // What is marked counts, if only one: a change of signs or spaces has no word.
    if (piece.op === 'removed') {
      this.removed += Math.max(1, weight(this.a, piece.from, piece.to, this.unit));
      this.#push(slice(this.a, piece.from, piece.to, 'removed'));
      return;
    }
    // A line of code removed with no end of its own: what is added after it starts a line.
    if (this.unit === 'lines' && this.#afterRemovedLine())
      this.content.push({ type: 'text', text: '\n', marks: [diffMark('break')] });
    this.added += Math.max(1, weight(this.b, piece.from, piece.to, this.unit));
    this.#push(slice(this.b, piece.from, piece.to, 'added'));
  }

  /** Whether the content so far ends with a removed line whose end is not in it. */
  #afterRemovedLine(): boolean {
    const last = this.content.at(-1);
    const removed = last?.marks?.some(
      (mark) => mark.type === DIFF_MARK && mark.attrs?.change === 'removed',
    );
    return removed === true && !(last?.text ?? '').endsWith('\n');
  }

  /** A hunk of runs between spaces: compared again word by word, unless it is too long to be. */
  words(hunk: Hunk, budget: DiffBudget): void {
    const { aFrom, aTo, bFrom, bTo } = spanOf(hunk);
    if (aTo - aFrom + (bTo - bFrom) > INLINE_LIMIT) {
      budget.spent = true;
      for (const piece of piecesOf(hunk, false)) this.put(piece);
      return;
    }
    const parts = compareTokens(
      tokensOf(this.a, aFrom, aTo, 'words'),
      tokensOf(this.b, bFrom, bTo, 'words'),
      budget,
    );
    for (const part of parts) {
      if ('op' in part) this.put(part);
      else for (const piece of piecesOf(part, true)) this.put(piece);
    }
  }
}

/** Where both texts start alike and end alike, cut back to where `cut` lets them be cut. */
function sharedEndsOf(
  a: Inline,
  b: Inline,
  cut: (text: string, at: number) => boolean,
): { prefix: number; suffix: number } {
  let prefix = 0;
  const shorter = Math.min(a.text.length, b.text.length);
  while (prefix < shorter && sameAt(a, prefix, b, prefix)) prefix += 1;
  let suffix = 0;
  while (
    suffix < shorter - prefix &&
    sameAt(a, a.text.length - 1 - suffix, b, b.text.length - 1 - suffix)
  )
    suffix += 1;
  while (prefix > 0 && !(cut(a.text, prefix) && cut(b.text, prefix))) prefix -= 1;
  while (
    suffix > 0 &&
    !(cut(a.text, a.text.length - suffix) && cut(b.text, b.text.length - suffix))
  )
    suffix -= 1;
  return { prefix, suffix };
}

const atLine = (text: string, at: number): boolean =>
  at === 0 || at === text.length || text[at - 1] === '\n';

/**
 * The inline content of block `after` against that of `before`: word by word
 * (`words`), or line by line (`lines`, for code). Words are found in two
 * passes: the runs between spaces first, which is quick over a long text,
 * then the words of the runs that differ.
 */
export function diffInline(
  before: BodyNode,
  after: BodyNode,
  unit: 'words' | 'lines',
  budget: DiffBudget,
): InlineDiff {
  const a = inlineOf(before);
  const b = inlineOf(after);
  const { prefix, suffix } = sharedEndsOf(a, b, unit === 'lines' ? atLine : atBreak);
  const merging = new Merging(a, b, unit);
  const aEnd = a.text.length - suffix;
  const bEnd = b.text.length - suffix;
  merging.same(0, 0, prefix);
  const parts = compareTokens(
    tokensOf(a, prefix, aEnd, unit === 'lines' ? 'lines' : 'chunks'),
    tokensOf(b, prefix, bEnd, unit === 'lines' ? 'lines' : 'chunks'),
    budget,
  );
  for (const part of parts) {
    if ('op' in part) merging.put(part);
    else if (unit === 'lines') for (const piece of piecesOf(part, false)) merging.put(piece);
    else merging.words(part, budget);
  }
  merging.same(aEnd, bEnd, suffix);
  const { content, changed, added, removed, formatted } = merging;
  return { content, changed, added, removed, formatted };
}
