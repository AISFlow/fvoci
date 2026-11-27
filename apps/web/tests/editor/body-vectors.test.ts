/**
 * The shared test vectors of a body (`contracts/body-vectors.json`): the
 * documents the server's test reads too (`crates/model/tests/body_vectors.rs`),
 * and what both sides must do with each. The web app's verdict is the one
 * its writes go by: the validator (`validateDoc`), the size limit, and an
 * empty document standing for no body.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { plainText } from '$lib/editor/body-text.ts';
import {
  BODY_VERSION,
  isEmptyDoc,
  MAX_BODY_BYTES,
  storedSize,
  validateDoc,
  type DocNode,
} from '$lib/editor/schema.ts';

interface Build {
  depth?: number;
  nodes?: number;
  bytes?: number;
  rows?: number;
  columns?: number;
}

interface Case {
  name: string;
  body?: unknown;
  build?: Build;
  expect: 'accept' | 'empty' | 'reject';
  reason?: string;
  text?: string;
}

const vectors = JSON.parse(
  readFileSync(
    path.resolve(import.meta.dirname, '../../../../contracts/body-vectors.json'),
    'utf8',
  ),
) as { bodyVersion: number; cases: Case[] };

/** A document as a case's `build` describes it (the vectors' own description says how). */
function build(spec: Build): DocNode {
  if (spec.rows !== undefined && spec.columns !== undefined) {
    const cell = { type: 'tableCell', content: [{ type: 'paragraph' }] };
    const row = { type: 'tableRow', content: Array.from({ length: spec.columns }, () => cell) };
    const rows = Array.from({ length: spec.rows }, () => row);
    return { type: 'doc', content: [{ type: 'table', content: rows }] };
  }
  if (spec.depth !== undefined) {
    let node: DocNode = { type: 'paragraph' };
    for (let level = 0; level < spec.depth - 2; level += 1) {
      node = { type: 'blockquote', content: [node] };
    }
    return { type: 'doc', content: [node] };
  }
  if (spec.nodes !== undefined) {
    const breaks = Array.from({ length: spec.nodes - 2 }, () => ({ type: 'hardBreak' }));
    return { type: 'doc', content: [{ type: 'paragraph', content: breaks }] };
  }
  if (spec.bytes !== undefined) {
    const frame = (text: string): DocNode => ({
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
    });
    const overhead = storedSize(frame(''));
    return frame('a'.repeat(spec.bytes - overhead));
  }
  throw new Error(`a build of nothing: ${JSON.stringify(spec)}`);
}

const mentions = (node: DocNode): boolean =>
  node.type === 'mention' || (node.content ?? []).some(mentions);

/** What the web app does with a body before it writes one. */
function verdict(body: unknown): 'accept' | 'empty' | 'reject' {
  if (body === null) return 'empty';
  if (validateDoc(body).length > 0) return 'reject';
  const doc = body as DocNode;
  if (storedSize(doc) > MAX_BODY_BYTES) return 'reject';
  return isEmptyDoc(doc) ? 'empty' : 'accept';
}

describe('the shared test vectors of a body', () => {
  it('are written for the version of the list the editor has', () => {
    expect(vectors.bodyVersion).toBe(BODY_VERSION);
    expect(vectors.cases.length).toBeGreaterThan(100);
  });

  it.each(vectors.cases.map((one) => [one.name, one] as const))('%s', (_name, one) => {
    const body = one.build ? build(one.build) : one.body;
    if (one.build?.bytes !== undefined) expect(storedSize(body as DocNode)).toBe(one.build.bytes);
    expect(verdict(body)).toBe(one.expect);
    // The words the server stores for searching are the web app's own, but for
    // a mention: the web app gives the name, the server nothing.
    if (one.text !== undefined && !mentions(body as DocNode))
      expect(plainText(body as DocNode)).toBe(one.text);
  });
});
