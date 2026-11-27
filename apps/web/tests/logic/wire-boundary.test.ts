/**
 * The rule of wire.ts, checked: no other module of the logic, and no test
 * outside the `wire*` files, looks inside the `fields` of a block or parses a stored value itself.
 */
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

const LOGIC = path.resolve(import.meta.dirname, '../../src/lib/logic');
const TESTS = import.meta.dirname;

function offenders(directory: string, allowed: RegExp, pattern: RegExp): string[] {
  const found: string[] = [];
  for (const file of readdirSync(directory)) {
    if (!file.endsWith('.ts') || allowed.test(file)) continue;
    readFileSync(path.join(directory, file), 'utf8')
      .split('\n')
      .forEach((line, index) => {
        if (pattern.test(line)) found.push(`${file}:${index + 1}: ${line.trim()}`);
      });
  }
  return found;
}

describe('only wire.ts knows the wire shapes', () => {
  it('no other logic module reads the fields of a block', () => {
    expect(offenders(LOGIC, /^wire(?:-|\.)/, /\.fields\b/)).toEqual([]);
  });

  it('no other logic module parses or writes JSON', () => {
    expect(offenders(LOGIC, /^wire(?:-|\.)/, /JSON\.(parse|stringify)/)).toEqual([]);
  });

  it('no test but those of wire.ts reads the fields of a block', () => {
    expect(offenders(TESTS, /^wire[.-]/, /\.fields\b/)).toEqual([]);
  });
});
