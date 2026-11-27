/**
 * The vectors of a body as a Y document (`contracts/ydoc-vectors.json`),
 * which the server's conversion is held against (`crates/collab`): that the
 * file is what the tool writes from the editor's schema and binding, and
 * what the file says about the two.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { getSchema } from '@tiptap/core';
import { Collaboration } from '@tiptap/extension-collaboration';
import { Node } from '@tiptap/pm/model';
import { describe, expect, it } from 'vitest';
import * as Y from 'yjs';

import { bodyExtensions } from '$lib/editor/extensions.ts';
import { BODY_VERSION, toStoredDoc, validateDoc, type DocNode } from '$lib/editor/schema.ts';

import {
  buildVectors,
  FIELD,
  fromBase64,
  readBodyCases,
  readerOf,
  readStored,
  serialize,
  YDOC_VECTORS_FILE,
  type YdocVectors,
} from '../../tools/ydoc/vectors.ts';
import { shapeOf } from '../../tools/ydoc/shape.ts';

const schema = getSchema(bodyExtensions());
const committed = readFileSync(YDOC_VECTORS_FILE, 'utf8');
const vectors = JSON.parse(committed) as YdocVectors;

/** Whether the editor's own strict check takes a document. */
function passesStrictCheck(json: DocNode): boolean {
  try {
    Node.fromJSON(schema, json).check();
    return true;
  } catch {
    return false;
  }
}

describe('the vectors of a body as a Y document', () => {
  it('are what the tool writes', () => {
    // Making them also holds each case to what it is filed under: an editor
    // keeps every document and reading as it is, and deletes from every repair.
    expect(serialize(buildVectors(schema, readBodyCases()))).toBe(committed);
  });

  it('are written for the version of the list the editor has, and name the shared type its collaboration extension binds to', () => {
    expect(vectors.bodyVersion).toBe(BODY_VERSION);
    expect(Collaboration.options.field).toBe(FIELD);
    expect(vectors.field).toBe(FIELD);
  });

  it('hold every accepted body, as an editor holds it', () => {
    const accepted = readBodyCases().filter(
      (one) => one.expect === 'accept' && one.body !== undefined,
    );
    expect(vectors.documents.map((one) => one.name)).toEqual(accepted.map((one) => one.name));
    expect(vectors.documents.length).toBeGreaterThan(40);
    for (const [index, one] of vectors.documents.entries()) {
      const held = toStoredDoc(Node.fromJSON(schema, accepted[index]?.body).toJSON() as DocNode);
      expect(one.json, one.name).toEqual(held);
    }
  });

  it('hold more bodies, each as an editor holds it', () => {
    expect(vectors.bodies.length).toBeGreaterThan(5);
    for (const one of vectors.bodies) {
      expect(validateDoc(one.body), one.name).toEqual([]);
      const held = toStoredDoc(Node.fromJSON(schema, one.body).toJSON() as DocNode);
      expect(one.json, one.name).toEqual(held);
    }
  });

  it.each(
    [...vectors.documents, ...vectors.bodies, ...vectors.readings, ...vectors.repairs].map(
      (one) => [one.name, one] as const,
    ),
  )('%s: an editor reads the update as the JSON beside it, which is a body', (_name, one) => {
    expect(readStored(readerOf(fromBase64(one.update)), schema)).toEqual(one.json);
    if (one.json !== null) expect(validateDoc(one.json)).toEqual([]);
  });

  it.each([...vectors.refused, ...vectors.shortened].map((one) => [one.name, one] as const))(
    '%s: an editor reads the update as the JSON beside it, which is no body',
    (_name, one) => {
      expect(readStored(readerOf(fromBase64(one.update)), schema)).toEqual(one.json);
      expect(validateDoc(one.json)).not.toEqual([]);
    },
  );

  it("the editor's strict check refuses what the binding keeps: code beside another mark", () => {
    // The server accepts such a body, and a merge of two edits can make one.
    // An editor made with its content check on would stop at it.
    const marksOf = (node: DocNode): string[][] => [
      ...(node.marks ? [node.marks.map((mark) => mark.type)] : []),
      ...(node.content ?? []).flatMap(marksOf),
    ];
    const hasCodeBesideAnother = (json: DocNode): boolean =>
      marksOf(json).some((marks) => marks.includes('code') && marks.length > 1);
    const kept = [...vectors.documents, ...vectors.bodies, ...vectors.readings].flatMap((one) =>
      one.json === null ? [] : [{ name: one.name, json: one.json }],
    );
    const refused = kept.filter((one) => !passesStrictCheck(one.json)).map((one) => one.name);
    expect(refused).toEqual(
      kept.filter((one) => hasCodeBesideAnother(one.json)).map((one) => one.name),
    );
    expect(refused).toEqual([
      'every mark once',
      'marks come in the order of the schema, however they were written',
      'bold and code together, which the editor never sets, are both kept',
    ]);
  });
});

/**
 * What the server's conversion makes (`crates/collab`), written by its own
 * test: the Y document of every body, and every repair's document after the
 * server has repaired it.
 */
const made = JSON.parse(
  readFileSync(
    path.resolve(import.meta.dirname, '../../../../crates/collab/tests/data/made-here.json'),
    'utf8',
  ),
) as Record<'documents' | 'repairs', { name: string; update: string }[]> & {
  mended: { name: string; update: string; json: DocNode | null }[];
};

describe('the Y documents the server makes', () => {
  const byName = (section: YdocVectors['documents']): Map<string, YdocVectors['documents'][0]> =>
    new Map(section.map((one) => [one.name, one]));
  const bodies = byName([...vectors.documents, ...vectors.bodies]);
  const repairs = byName(vectors.repairs);

  /** Reads as an editor reads, and says whether the editor deleted anything while it did. */
  function read(update: string): { json: DocNode | null; deleted: boolean; doc: Y.Doc } {
    const doc = readerOf(fromBase64(update));
    const before = Y.encodeStateAsUpdate(doc);
    const json = readStored(doc, schema);
    const after = Y.encodeStateAsUpdate(doc);
    const deleted = before.length !== after.length || before.some((byte, at) => byte !== after[at]);
    return { json, deleted, doc };
  }

  it('are one for every body and every repair of the vectors', () => {
    expect(made.documents.map((one) => one.name)).toEqual([...bodies.keys()]);
    expect(made.repairs.map((one) => one.name)).toEqual([...repairs.keys()]);
    // All but what is past a limit, which the server refuses and does not repair.
    expect(made.mended.map((one) => one.name)).toEqual(
      [...vectors.refused, ...vectors.shortened]
        .map((one) => one.name)
        .filter((name) => name !== 'a table of twenty-one columns'),
    );
  });

  it.each(made.documents.map((one) => [one.name, one] as const))(
    '%s: an editor reads the same from it as from the document of its own binding, which has the same shape',
    (name, one) => {
      const theirs = bodies.get(name);
      const mine = read(one.update);
      expect(mine.json).toEqual(theirs?.json);
      expect(mine.deleted).toBe(false);
      expect(shapeOf(mine.doc.getXmlFragment(FIELD))).toEqual(
        shapeOf(readerOf(fromBase64(theirs?.update ?? '')).getXmlFragment(FIELD)),
      );
    },
  );

  it.each(made.repairs.map((one) => [one.name, one] as const))(
    '%s: after the repair an editor reads what its own repair leaves, and deletes nothing more',
    (name, one) => {
      const mine = read(one.update);
      expect(mine.json).toEqual(repairs.get(name)?.json);
      expect(mine.deleted).toBe(false);
    },
  );

  it.each(made.mended.map((one) => [one.name, one] as const))(
    '%s: after the repair an editor reads the body the server reads, and deletes nothing',
    (_name, one) => {
      const mine = read(one.update);
      expect(mine.json).toEqual(one.json);
      if (one.json !== null) expect(validateDoc(one.json)).toEqual([]);
      expect(mine.deleted).toBe(false);
    },
  );
});
