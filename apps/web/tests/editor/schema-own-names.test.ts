/**
 * The list of what a body may contain is looked up by name. A name that every
 * object has a member for (`constructor`, `toString`, `__proto__`) is not on
 * the list just because the lookup finds something.
 */
import { describe, expect, it } from 'vitest';

import { MARK_SPECS, NODE_SPECS, validateDoc, type DocNode } from '$lib/editor/schema.ts';

const INHERITED = ['constructor', 'toString', 'valueOf', 'hasOwnProperty', '__proto__'];

const withMark = (type: string): DocNode => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text: 'x', marks: [{ type }] }] }],
});

describe('names that are not on the list', () => {
  it.each(INHERITED)('refuses a mark named %s', (name) => {
    expect(Object.hasOwn(MARK_SPECS, name)).toBe(false);
    expect(validateDoc(withMark(name))).toEqual([
      `/doc[0]/paragraph[0]/text: unknown mark "${name}"`,
    ]);
  });

  it.each(INHERITED)('refuses a node named %s', (name) => {
    expect(Object.hasOwn(NODE_SPECS, name)).toBe(false);
    expect(validateDoc({ type: 'doc', content: [{ type: name }] })).toEqual([
      `/doc[0]: unknown node type "${name}"`,
    ]);
    expect(validateDoc({ type: name })).toEqual([`: unknown node type "${name}"`]);
  });

  it('still accepts every mark that is on the list', () => {
    for (const name of ['bold', 'italic', 'strike', 'code'])
      expect(validateDoc(withMark(name))).toEqual([]);
  });
});
