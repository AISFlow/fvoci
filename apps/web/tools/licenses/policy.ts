/**
 * The licence policy of the web app.
 *
 * The list of acceptable licences is not kept here: it is read from the
 * `deny.toml` at the repository root, the file that `cargo deny` checks the
 * server's dependencies against, so both halves of the product follow one list.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

export const REPOSITORY_ROOT = path.resolve(import.meta.dirname, '../../../..');
export const DENY_TOML = path.join(REPOSITORY_ROOT, 'deny.toml');

/** Reads `allow = [...]` of the `[licenses]` table of a cargo-deny file. */
export function readAllowedLicenses(file: string = DENY_TOML): string[] {
  const text = readFileSync(file, 'utf8');
  const table = /^\[licenses\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(text);
  if (!table?.[1]) throw new Error(`${file}: no [licenses] table`);
  const list = /^allow\s*=\s*\[([\s\S]*?)\]/m.exec(table[1]);
  if (!list?.[1]) throw new Error(`${file}: no allow list in [licenses]`);
  const names = [...list[1].matchAll(/"([^"]+)"/g)].map((match) => match[1] ?? '');
  if (names.length === 0) throw new Error(`${file}: the allow list is empty`);
  return names;
}

type Token = '(' | ')' | 'AND' | 'OR' | 'WITH' | { id: string };

function tokenize(expression: string): Token[] {
  const tokens: Token[] = [];
  for (const [raw] of expression.matchAll(/\(|\)|[^\s()]+/g)) {
    const upper = raw.toUpperCase();
    if (raw === '(' || raw === ')') tokens.push(raw);
    else if (upper === 'AND' || upper === 'OR' || upper === 'WITH') tokens.push(upper);
    else tokens.push({ id: raw });
  }
  return tokens;
}

/**
 * Whether an SPDX licence expression is acceptable under the given list.
 *
 * `A OR B` needs one side, `A AND B` needs both. `A WITH E` is one licence and
 * has to be on the list in that exact form, as it is in `deny.toml`. Anything
 * that is not a well-formed expression (a free text such as `SEE LICENSE IN`)
 * is not acceptable.
 */
export function isAllowed(expression: string, allowed: ReadonlySet<string>): boolean {
  const tokens = tokenize(expression);
  let position = 0;

  const parseAtom = (): boolean | undefined => {
    const token = tokens[position];
    if (token === '(') {
      position += 1;
      const inner = parseOr();
      if (tokens[position] !== ')') return undefined;
      position += 1;
      return inner;
    }
    if (typeof token !== 'object') return undefined;
    position += 1;
    let name = token.id;
    if (tokens[position] === 'WITH') {
      const exception = tokens[position + 1];
      if (typeof exception !== 'object') return undefined;
      name = `${name} WITH ${exception.id}`;
      position += 2;
    }
    return allowed.has(name);
  };

  const parseAnd = (): boolean | undefined => {
    let result = parseAtom();
    while (result !== undefined && tokens[position] === 'AND') {
      position += 1;
      const right = parseAtom();
      result = right === undefined ? undefined : result && right;
    }
    return result;
  };

  function parseOr(): boolean | undefined {
    let result = parseAnd();
    while (result !== undefined && tokens[position] === 'OR') {
      position += 1;
      const right = parseAnd();
      result = right === undefined ? undefined : result || right;
    }
    return result;
  }

  const result = parseOr();
  return result === true && position === tokens.length;
}
