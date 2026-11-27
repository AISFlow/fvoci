/**
 * A stored body as the comparison of two of them sees it (body-diff.ts): the
 * shape of a document (editor/schema.ts's `DocNode`, which this layer cannot
 * name), made canonical so that two bodies that say the same are equal
 * whoever stored them, and a short key for each node.
 */

export interface BodyMark {
  type: string;
  attrs?: Record<string, unknown>;
}

export interface BodyNode {
  type: string;
  attrs?: Record<string, unknown>;
  content?: BodyNode[];
  text?: string;
  marks?: BodyMark[];
}

/**
 * What a node of the comparison is: in the second body only (`added`), in
 * the first only (`removed`), or in both with something in it or about it
 * changed (`changed`). A node with none of these is the same in both.
 */
export type DiffChange = 'added' | 'removed' | 'changed';

/** A node of the merged body: the second body's, and the first's where it had something else. */
export interface DiffNode extends BodyNode {
  diff?: DiffChange;
  /** Of a node changed: its kind and attributes in the first body, when they were others. */
  was?: { type: string; attrs?: Record<string, unknown> };
  content?: DiffNode[];
}

/** The mark that says what became of a piece of inline content: `{ type: 'diff', attrs: { change } }`. */
export const DIFF_MARK = 'diff';

/**
 * What became of a piece of inline content: added, removed, or the same text
 * with other marks. A `break` is in neither body: the end of a line put
 * between the last line of code removed and the lines added after it, so
 * that the two do not run together on one line.
 */
export type InlineChange = 'added' | 'removed' | 'format' | 'break';

/** The kinds of block whose content is inline. */
export const TEXTBLOCKS = new Set(['paragraph', 'heading', 'codeBlock']);

/** The kinds of node that are one thing and not text: each counts as one word. */
const ATOMS = new Set([
  'image',
  'file',
  'blockMath',
  'horizontalRule',
  'mention',
  'reference',
  'inlineMath',
]);

/** Attributes in an order of their names, and without the ones that mean nothing. */
function canonicalAttrs(node: BodyNode): Record<string, unknown> | undefined {
  const entries = Object.entries(node.attrs ?? {})
    .filter(
      ([name, value]) =>
        value !== null &&
        value !== undefined &&
        !(node.type === 'orderedList' && name === 'start' && value === 1),
    )
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return entries.length > 0 ? Object.fromEntries(entries) : undefined;
}

/**
 * A text that is the same for two values exactly when they are equal, the
 * names of an object's fields in any order: what attributes are compared by.
 * A string is written with its length, so that no text inside one can be
 * taken for the end of it.
 */
export function valueKey(value: unknown): string {
  if (typeof value === 'string') return `s${String(value.length)}:${value}`;
  if (value === null || typeof value !== 'object') return `${typeof value}:${String(value)}`;
  if (Array.isArray(value)) return `[${value.map(valueKey).join(',')}]`;
  const record = value as Record<string, unknown>;
  const fields = Object.keys(record)
    .sort()
    .map((name) => `${valueKey(name)}=${valueKey(record[name])}`);
  return `{${fields.join(',')}}`;
}

const markKey = (mark: BodyMark): string =>
  mark.type + (mark.attrs ? valueKey(canonicalAttrs({ type: '', attrs: mark.attrs })) : '');

/** The marks of a text as a set: in one order, each once. */
function canonicalMarks(marks: BodyMark[] | undefined): BodyMark[] | undefined {
  if (!marks || marks.length === 0) return undefined;
  const byKey = new Map<string, BodyMark>();
  for (const mark of marks) {
    const attrs = canonicalAttrs({ type: '', attrs: mark.attrs ?? {} });
    byKey.set(markKey(mark), attrs ? { type: mark.type, attrs } : { type: mark.type });
  }
  return [...byKey.entries()].sort(([a], [b]) => (a < b ? -1 : 1)).map(([, mark]) => mark);
}

/** The key of a set of marks, `''` for none. */
export const marksKey = (marks: BodyMark[] | undefined): string =>
  (marks ?? []).map(markKey).join('\u0001');

/**
 * A node as any store would hold it: attributes without the ones that mean
 * nothing and in one order, marks as a set, neighbouring texts of the same
 * marks as one, and no empty text.
 */
export function canonical(node: BodyNode): BodyNode {
  const out: BodyNode = { type: node.type };
  const attrs = canonicalAttrs(node);
  if (attrs) out.attrs = attrs;
  if (node.type === 'text') {
    out.text = node.text ?? '';
    const marks = canonicalMarks(node.marks);
    if (marks) out.marks = marks;
    return out;
  }
  if (node.content) {
    const content: BodyNode[] = [];
    for (const child of node.content) {
      const made = canonical(child);
      if (made.type === 'text' && made.text === '') continue;
      const last = content.at(-1);
      if (
        last?.type === 'text' &&
        made.type === 'text' &&
        marksKey(last.marks) === marksKey(made.marks)
      )
        last.text = (last.text ?? '') + (made.text ?? '');
      else content.push(made);
    }
    out.content = content;
  }
  return out;
}

/** The blocks of a body: none for no body, or for the one empty paragraph an empty body is. */
export function blocksOf(body: BodyNode | null | undefined): BodyNode[] {
  if (!body) return [];
  const blocks = canonical(body).content ?? [];
  const [only, ...others] = blocks;
  if (others.length === 0 && only?.type === 'paragraph' && (only.content ?? []).length === 0)
    return [];
  return blocks;
}

/** A short sum of a text (cyrb53): 53 bits, as 11 letters. */
export function sum(text: string): string {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    h1 = Math.imul(h1 ^ code, 2654435761);
    h2 = Math.imul(h2 ^ code, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return (4294967296 * (2097151 & h2) + (h1 >>> 0)).toString(36).padStart(11, '0');
}

/**
 * The keys of canonical nodes: equal for nodes that are equal, all through.
 * Each is a sum of the node's own part and the keys of its children, so that
 * a body is keyed in one pass however deep it is.
 */
export class NodeKeys {
  readonly #keys = new WeakMap<BodyNode, string>();

  of(node: BodyNode): string {
    const known = this.#keys.get(node);
    if (known !== undefined) return known;
    let own = node.type;
    if (node.attrs) own += valueKey(node.attrs);
    if (node.text !== undefined) own += `\u0002${node.text}\u0002${marksKey(node.marks)}`;
    for (const child of node.content ?? []) own += this.of(child);
    const key = sum(own);
    this.#keys.set(node, key);
    return key;
  }
}

const WORDS = /[\p{L}\p{N}\p{M}]+(?:['’][\p{L}\p{N}\p{M}]+)*/gu;

/** How many words a text has, for the counts of what was added and removed. */
export function wordCount(text: string): number {
  let count = 0;
  WORDS.lastIndex = 0;
  while (WORDS.exec(text)) count += 1;
  return count;
}

/** The text of a node and all in it, inline things that are not text as a space. */
export function textOf(node: BodyNode): string {
  if (node.type === 'text') return node.text ?? '';
  return (node.content ?? []).map(textOf).join(node.type === 'paragraph' ? '' : ' ');
}

/** Whether a node that is not text counts as one word: a line break, which is neither, counts nothing. */
export const countsOne = (type: string): boolean => ATOMS.has(type);

/**
 * How much a node and all in it counts, for the counts of what was added and
 * removed: its words, and one for each thing that is not text (an image, a
 * formula, a mention, a diagram: a code block of `diagram`, its source no words).
 */
export function weightOf(node: BodyNode, diagram?: string): number {
  if (node.type === 'text') return wordCount(node.text ?? '');
  if (ATOMS.has(node.type)) return 1;
  if (diagram !== undefined && node.type === 'codeBlock' && node.attrs?.language === diagram)
    return 1;
  let weight = 0;
  for (const child of node.content ?? []) weight += weightOf(child, diagram);
  return weight;
}
