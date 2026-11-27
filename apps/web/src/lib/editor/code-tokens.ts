/**
 * The colours of a code block's text, as one list both the editor and the
 * read-only view draw from.
 *
 * The highlighter (highlight.js, through lowlight) gives a tree of scopes:
 * `keyword`, `title.function`, a `string` with a `subst` inside. That tree is
 * read here into runs of the text, each of one kind of ours. The kinds are
 * few and are the colours of the style sheet (code-block.css, the tokens of
 * app.css); the highlighter's own names stay here. A run takes the kind of
 * the innermost scope it is in that has one: the inside of `${…}` in a string
 * is code again, not string.
 *
 * Tokens are a flat list of numbers, three to a run: where it starts and
 * where it ends in the text (in UTF-16 code units, as the editor counts), and
 * the index of its kind. Runs are in order, do not overlap and never touch
 * with the same kind. Text between runs has no colour.
 */

/**
 * What the highlighter's worker says once it is loaded and can take texts
 * (code-highlight-worker.ts): the page's time for a text starts then.
 */
export const HIGHLIGHT_READY = 'ready';

/** The kinds of text that have a colour of their own. */
export const TOKEN_KINDS = [
  'comment',
  'keyword',
  'string',
  'number',
  'title',
  'type',
  'builtin',
  'property',
  'variable',
  'meta',
  'regexp',
  'tag',
  'added',
  'removed',
] as const;

export type TokenKind = (typeof TOKEN_KINDS)[number];

/** Flat triples: start, end, index into `TOKEN_KINDS`. */
export type CodeTokens = readonly number[];

/** The class of the element that holds a run of this kind. */
export const tokenClass = (kind: TokenKind): string => `doc-code-${kind}`;

/** The class of a run, by the index the tokens carry. */
export function tokenClassAt(index: number): string | undefined {
  const kind = TOKEN_KINDS[index];
  return kind === undefined ? undefined : tokenClass(kind);
}

/**
 * The kind of each scope of the highlighter, by the first part of its name.
 * `null` starts plain text again (the inside of a `${…}`); a scope not named
 * here takes the kind of the one around it. A scope's second part (the
 * `function` of `title.function`) does not change its kind, except where
 * `SUBSCOPE_KINDS` says.
 */
const SCOPE_KINDS: Readonly<Record<string, TokenKind | null>> = {
  comment: 'comment',
  doctag: 'comment',
  quote: 'comment',
  keyword: 'keyword',
  'selector-tag': 'keyword',
  bullet: 'keyword',
  string: 'string',
  char: 'string',
  symbol: 'string',
  link: 'string',
  code: 'string',
  number: 'number',
  literal: 'number',
  title: 'title',
  section: 'title',
  'selector-id': 'title',
  'selector-class': 'title',
  type: 'type',
  built_in: 'builtin',
  attr: 'property',
  attribute: 'property',
  property: 'property',
  'selector-attr': 'property',
  'selector-pseudo': 'property',
  variable: 'variable',
  'template-variable': 'variable',
  meta: 'meta',
  regexp: 'regexp',
  name: 'tag',
  addition: 'added',
  deletion: 'removed',
  subst: null,
  params: null,
  operator: null,
  punctuation: null,
};

/** Where the second part of a scope's name gives it another kind: `variable.language` is `this`. */
const SUBSCOPE_KINDS: Readonly<Record<string, TokenKind>> = {
  'variable language_': 'builtin',
};

/** A node of the highlighter's tree, as much of it as is read here. */
interface TreeNode {
  type: string;
  value?: string;
  properties?: { className?: unknown };
  children?: readonly TreeNode[];
}

const PREFIX = 'hljs-';

function kindOf(node: TreeNode, around: TokenKind | null): TokenKind | null {
  const names = node.properties?.className;
  if (!Array.isArray(names)) return around;
  const [first, second] = names as unknown[];
  if (typeof first !== 'string' || !first.startsWith(PREFIX)) return around;
  const scope = first.slice(PREFIX.length);
  const special = typeof second === 'string' ? SUBSCOPE_KINDS[`${scope} ${second}`] : undefined;
  if (special) return special;
  const kind = SCOPE_KINDS[scope];
  return kind === undefined ? around : kind;
}

/** The tokens of a tree the highlighter made of a text. */
export function tokensOfTree(tree: TreeNode): number[] {
  const tokens: number[] = [];
  let at = 0;
  const visit = (node: TreeNode, around: TokenKind | null): void => {
    if (node.type === 'text') {
      const length = node.value?.length ?? 0;
      if (length === 0) return;
      if (around !== null) {
        const kind = TOKEN_KINDS.indexOf(around);
        const last = tokens.length - 3;
        // A run that goes on where the last one of its kind ended is that run.
        if (last >= 0 && tokens[last + 1] === at && tokens[last + 2] === kind)
          tokens[last + 1] = at + length;
        else tokens.push(at, at + length, kind);
      }
      at += length;
      return;
    }
    const kind = node.type === 'element' ? kindOf(node, around) : around;
    for (const child of node.children ?? []) visit(child, kind);
  };
  visit(tree, null);
  return tokens;
}

/**
 * Whether a value is tokens for a text of this length: what a worker sends
 * is read again here before it is drawn.
 */
export function isCodeTokens(value: unknown, length: number): value is CodeTokens {
  if (!Array.isArray(value) || value.length % 3 !== 0) return false;
  let end = 0;
  for (let index = 0; index < value.length; index += 3) {
    const [from, to, kind] = [value[index], value[index + 1], value[index + 2]] as unknown[];
    if (!Number.isInteger(from) || !Number.isInteger(to) || !Number.isInteger(kind)) return false;
    const [start, stop, which] = [from as number, to as number, kind as number];
    if (start < end || stop <= start || stop > length) return false;
    if (which < 0 || which >= TOKEN_KINDS.length) return false;
    end = stop;
  }
  return true;
}

/**
 * The tokens of an older text moved onto a newer one, for the moment until
 * the newer one is coloured: runs before the change stay, runs after it move
 * with the text, and what changed is plain. The change is the stretch where
 * the two texts differ, found from both ends.
 */
export function moveTokens(tokens: CodeTokens, before: string, after: string): number[] {
  let head = 0;
  const shortest = Math.min(before.length, after.length);
  while (head < shortest && before.charCodeAt(head) === after.charCodeAt(head)) head += 1;
  let tail = 0;
  while (
    tail < shortest - head &&
    before.charCodeAt(before.length - 1 - tail) === after.charCodeAt(after.length - 1 - tail)
  )
    tail += 1;
  const oldEnd = before.length - tail;
  const shift = after.length - before.length;
  const moved: number[] = [];
  for (let index = 0; index + 2 < tokens.length; index += 3) {
    const [from = 0, to = 0, kind = 0] = [tokens[index], tokens[index + 1], tokens[index + 2]];
    // The part before the change, and the part after it, of each run.
    if (from < head) moved.push(from, Math.min(to, head), kind);
    if (to > oldEnd) moved.push(Math.max(from, oldEnd) + shift, to + shift, kind);
  }
  return moved;
}
