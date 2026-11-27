/**
 * What changed from one body to another, as one body to read: the second
 * body's blocks, the first's that are gone from it among them, and each
 * block that is in both but changed marked so, down to the words.
 *
 * Blocks are matched first: the blocks of the body, then within a block in
 * both (a list, a quote, an item) its own, by what they hold all through
 * (`NodeKeys`). Of the blocks that do not match between two that do, one
 * gone and one come in their place are taken for the same block changed when
 * they are of a kind (a paragraph or a heading, a code block, a list, an item)
 * and say mostly the same, or when the stretch has as many of the one as of
 * the other, kind for kind. A block changed in place has its text compared
 * word by word, a code block's line by line (inline-diff.ts), and its kind
 * and attributes kept as they were (`was`: a heading's level, a code block's
 * language, a task's check). Anything else gone is `removed`, come `added`:
 * a block moved is one of each. A diagram, a formula, a file, a picture of
 * another file are matched whole or not at all.
 *
 * The comparison spends a budget of steps (sequence-diff.ts) over the whole
 * body, so that the largest bodies there can be (8 MiB, 100,000 nodes) are
 * compared in a moment: where it runs out, a stretch is removed and added
 * whole (`coarse`). See `DIFF_BUDGET`.
 */
import {
  blocksOf,
  NodeKeys,
  textOf,
  valueKey,
  weightOf,
  type BodyNode,
  type DiffNode,
} from './body-nodes.ts';
import { diffInline } from './inline-diff.ts';
import { DiffBudget, diffSequence, type DiffRun } from './sequence-diff.ts';

/**
 * The steps of the search one comparison of two bodies may take. Measured on
 * the largest bodies the server takes (tests/logic/body-diff-size.test.ts):
 * this many is about a fifth of a second at the most.
 */
export const DIFF_BUDGET = 20_000_000;

/** How far after the last block paired a gone block's match is looked for. */
const LOOK_AHEAD = 8;
/** How much of a block's text says what it is about, when two are weighed against each other. */
const LIKENESS_TEXT = 2000;

export interface BodyDiffOptions {
  /** A code block of this language is a diagram: compared whole, its lines meaning nothing alone. */
  diagramLanguage?: string;
  /** In steps of the search; `DIFF_BUDGET` unless given. */
  budget?: number;
}

/** What changed from one body to another. */
export interface BodyDiff {
  /** The merged body's blocks. */
  blocks: DiffNode[];
  /** Whether anything differs. */
  changed: boolean;
  /** Words (and things that are not text) added, removed, and of other marks only. */
  added: number;
  removed: number;
  formatted: number;
  /** Blocks of another kind or other attributes than they were (`was`). */
  reshaped: number;
  /** Some of it was compared whole for want of time: true, only coarser. */
  coarse: boolean;
}

/** What blocks may be taken for the same block changed: of the same family, which this names. */
function familyOf(node: BodyNode, diagram: string | undefined): string | undefined {
  switch (node.type) {
    case 'paragraph':
    case 'heading':
      return 'text';
    case 'codeBlock':
      return diagram !== undefined && node.attrs?.language === diagram ? undefined : 'code';
    case 'bulletList':
    case 'orderedList':
      return 'list';
    case 'taskList':
    case 'blockquote':
    case 'listItem':
    case 'taskItem':
      return node.type;
    case 'image':
      return `image ${String(node.attrs?.fileId)}`;
    default:
      return undefined;
  }
}

/** The words of the start of a block's text, as a bag. */
function bagOf(node: BodyNode): Map<string, number> {
  const bag = new Map<string, number>();
  for (const word of textOf(node).slice(0, LIKENESS_TEXT).split(/\s+/)) {
    if (word !== '') bag.set(word, (bag.get(word) ?? 0) + 1);
  }
  return bag;
}

/** How much two blocks say the same, from 0 to 1 (Dice's coefficient of their words). */
function likeness(a: BodyNode, b: BodyNode): number {
  const wa = bagOf(a);
  const wb = bagOf(b);
  let total = 0;
  let shared = 0;
  for (const [word, count] of wa) {
    total += count;
    shared += Math.min(count, wb.get(word) ?? 0);
  }
  for (const count of wb.values()) total += count;
  return total === 0 ? 1 : (2 * shared) / total;
}

const sameAttrs = (a: BodyNode, b: BodyNode): boolean =>
  valueKey(a.attrs ?? {}) === valueKey(b.attrs ?? {});

class Comparing {
  readonly budget: DiffBudget;
  readonly keys = new NodeKeys();
  readonly diagram: string | undefined;
  added = 0;
  removed = 0;
  formatted = 0;
  reshaped = 0;

  constructor(options: BodyDiffOptions) {
    this.budget = new DiffBudget(options.budget ?? DIFF_BUDGET);
    this.diagram = options.diagramLanguage;
  }

  #family(node: BodyNode): string | undefined {
    return familyOf(node, this.diagram);
  }

  /**
   * Which of a stretch's gone blocks are which of its come ones, changed:
   * pairs of their places, in order. Those that say mostly the same first;
   * between two such pairs, as many gone as come, kind for kind, each in its
   * place (a text rewritten where it stood).
   */
  #pairs(gone: BodyNode[], come: BodyNode[]): [number, number][] {
    const pairs: [number, number][] = [];
    let g = 0;
    let c = 0;
    const end: [number, number] = [gone.length, come.length];
    for (const [at, to] of [...this.#alike(gone, come), end]) {
      if (at - g === to - c && this.#kindForKind(gone.slice(g, at), come.slice(c, to)))
        for (let index = 0; index < at - g; index += 1) pairs.push([g + index, c + index]);
      if (at < gone.length) pairs.push([at, to]);
      g = at + 1;
      c = to + 1;
    }
    return pairs;
  }

  /** Whether two runs of blocks are of one family each, place for place. */
  #kindForKind(gone: BodyNode[], come: BodyNode[]): boolean {
    return gone.every((node, index) => {
      const family = this.#family(node);
      const other = come[index];
      return family !== undefined && other !== undefined && this.#family(other) === family;
    });
  }

  /** Each gone block with the first come one a little after the last taken that says mostly the same. */
  #alike(gone: BodyNode[], come: BodyNode[]): [number, number][] {
    const pairs: [number, number][] = [];
    let next = 0;
    gone.forEach((node, index) => {
      const family = this.#family(node);
      if (family === undefined) return;
      for (let at = next; at < Math.min(come.length, next + LOOK_AHEAD); at += 1) {
        const other = come[at];
        if (other && this.#family(other) === family && likeness(node, other) >= 0.5) {
          pairs.push([index, at]);
          next = at + 1;
          return;
        }
      }
    });
    return pairs;
  }

  /** A block removed whole: what is marked counts, if only one (an empty paragraph). */
  #gone(node: BodyNode): DiffNode {
    this.removed += Math.max(1, weightOf(node, this.diagram));
    return { ...node, diff: 'removed' };
  }

  #come(node: BodyNode): DiffNode {
    this.added += Math.max(1, weightOf(node, this.diagram));
    return { ...node, diff: 'added' };
  }

  /** One block in both, changed: the second's, with what changed in it. */
  #pair(before: BodyNode, after: BodyNode): DiffNode {
    const reshaped = before.type !== after.type || !sameAttrs(before, after);
    const family = this.#family(after);
    let node: DiffNode = after;
    let changed = reshaped;
    if (family === 'text' || family === 'code') {
      const inline = diffInline(before, after, family === 'code' ? 'lines' : 'words', this.budget);
      this.added += inline.added;
      this.removed += inline.removed;
      this.formatted += inline.formatted;
      node = { ...after, content: inline.content };
      changed ||= inline.changed;
    } else if (after.content) {
      const content = this.children(before.content ?? [], after.content);
      node = { ...after, content };
      changed ||= content.some((child) => child.diff !== undefined);
    }
    if (!changed) return after;
    if (!reshaped) return { ...node, diff: 'changed' };
    this.reshaped += 1;
    const was: DiffNode['was'] = { type: before.type };
    if (before.attrs) was.attrs = before.attrs;
    return { ...node, diff: 'changed', was };
  }

  /** A stretch that differs: its gone and come blocks, those taken for one changed in their place. */
  #stretch(gone: BodyNode[], come: BodyNode[], out: DiffNode[]): void {
    let g = 0;
    let c = 0;
    for (const [at, to] of this.#pairs(gone, come)) {
      for (const node of gone.slice(g, at)) out.push(this.#gone(node));
      for (const node of come.slice(c, to)) out.push(this.#come(node));
      const before = gone[at];
      const after = come[to];
      if (before && after) out.push(this.#pair(before, after));
      g = at + 1;
      c = to + 1;
    }
    for (const node of gone.slice(g)) out.push(this.#gone(node));
    for (const node of come.slice(c)) out.push(this.#come(node));
  }

  /** The merged blocks of two lists of blocks. */
  children(before: BodyNode[], after: BodyNode[]): DiffNode[] {
    const runs = diffSequence(
      before.map((node) => this.keys.of(node)),
      after.map((node) => this.keys.of(node)),
      this.budget,
    );
    const out: DiffNode[] = [];
    let stretch: { gone: BodyNode[]; come: BodyNode[] } | undefined;
    const close = (): void => {
      if (stretch) this.#stretch(stretch.gone, stretch.come, out);
      stretch = undefined;
    };
    const part = (list: BodyNode[], run: DiffRun, from: number): BodyNode[] =>
      list.slice(from, from + run.length);
    for (const run of runs) {
      if (run.step === 'same') {
        close();
        out.push(...part(after, run, run.to));
        continue;
      }
      stretch ??= { gone: [], come: [] };
      if (run.step === 'removed') stretch.gone.push(...part(before, run, run.from));
      else stretch.come.push(...part(after, run, run.to));
    }
    close();
    return out;
  }
}

/** What changed from `before` to `after`, two stored bodies (`null` for none). */
export function diffBodies(
  before: BodyNode | null | undefined,
  after: BodyNode | null | undefined,
  options: BodyDiffOptions = {},
): BodyDiff {
  const comparing = new Comparing(options);
  const blocks = comparing.children(blocksOf(before), blocksOf(after));
  return {
    blocks,
    changed: blocks.some((block) => block.diff !== undefined),
    added: comparing.added,
    removed: comparing.removed,
    formatted: comparing.formatted,
    reshaped: comparing.reshaped,
    coarse: comparing.budget.spent,
  };
}
