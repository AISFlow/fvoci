/**
 * Writes `contracts/ydoc-vectors.json`: bodies as the Y documents the
 * editor's binding makes of them, for the server's conversion
 * (`crates/collab`) to be held against.
 *
 *   node tools/ydoc/vectors.ts           writes the file
 *   node tools/ydoc/vectors.ts --check   fails when the file is not what would be written
 *
 * The server keeps a body as JSON and, while people edit it together, as a Y
 * document; it converts between the two without the editor's library. What
 * that library does is written down here by running it:
 *
 * - `documents`: every body the shared vectors (`contracts/body-vectors.json`)
 *   say is accepted, as the update of the Y document made from it, and the
 *   JSON an editor reads from that document in the form that is stored.
 * - `bodies`: the same for bodies written here (cases.ts), each with the body.
 * - `readings`: shared content no editor writes, which every editor that did
 *   not write it keeps as it is, and the JSON read from it.
 * - `refused`: shared content the editors keep the same way though what they
 *   read is no body; only the server's rules refuse it.
 * - `repairs`: shared content that does not fit the schema, and the JSON of
 *   what is left once an editor has deleted what fails.
 * - `session`: one body edited step by step, as the document the server opens
 *   and the updates the binding then sends, one after another.
 * - `schema`: the marks in the schema's order and the attributes each node
 *   and mark has, with the value an editor gives one that is left out.
 *
 * A document is made by a client whose identifier is fixed, so that the
 * file is the same whenever it is written.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import { Node, type Schema } from '@tiptap/pm/model';
import { prosemirrorToYXmlFragment, yXmlFragmentToProseMirrorRootNode } from '@tiptap/y-tiptap';
import * as Y from 'yjs';

import {
  BODY_VERSION,
  isEmptyDoc,
  toStoredDoc,
  validateDoc,
  type DocNode,
} from '../../src/lib/editor/schema.ts';

import { BODIES, READINGS, REFUSED, REPAIRS, SESSION, type RawCase } from './cases.ts';
import {
  DETAILS_MERGED_REFUSED,
  DETAILS_MERGED_REPAIRS,
  DETAILS_READINGS,
  DETAILS_REFUSED,
  DETAILS_REPAIRS,
} from './details-cases.ts';
import { rawType, type RawNode } from './shape.ts';
import {
  TABLE_MERGED_REFUSED,
  TABLE_MERGED_SHORTENED,
  TABLE_REFUSED,
  TABLE_REPAIRS,
  TABLE_SHORTENED,
  type MergedCase,
} from './table-cases.ts';

/** The shared type an editor is bound to: the collaboration extension's own name for it. */
export const FIELD = 'default';

const CONTRACTS = path.resolve(import.meta.dirname, '../../../../contracts');
export const BODY_VECTORS_FILE = path.join(CONTRACTS, 'body-vectors.json');
export const YDOC_VECTORS_FILE = path.join(CONTRACTS, 'ydoc-vectors.json');

const DESCRIPTION =
  'Bodies as Y documents, written by apps/web/tools/ydoc/vectors.ts from the ' +
  "editor's own binding (@tiptap/y-tiptap); the server's conversion (crates/collab) and the web " +
  "app's tests read it. Do not edit by hand. field is the name of the XML fragment at the root of " +
  'the Y document. An update is a whole Y document as a version 1 update in base64, and json is ' +
  'what an editor reads from it in the form that is stored (null for no body). documents: every ' +
  'accepted case of body-vectors.json with a body (not those with a build), by its name. bodies: ' +
  'more bodies, each with the body. In both, json is the body as it comes back: without an ' +
  'empty content or attrs, with the marks of a text in the order of the schema, neighbouring ' +
  'texts with the same marks as one, and an attribute the editor fills in (the start of an ' +
  'ordered list). readings: shared content no editor writes, which every editor that did not ' +
  'write it keeps as it is, and how it is read. refused: shared content the editors keep the ' +
  "same way though what they read (json) is no body: the editor's schema checks no value, the " +
  "server's rules do. repairs: shared content that does not fit the schema; json is what is left " +
  'when an editor has deleted what fails. shortened: shared content from which an editor ' +
  'deletes a cell that fails, which leaves its row a cell short, so that json is no body; the ' +
  'server puts an empty cell in its place. session: start is the document a server opens, ' +
  'updates are what the binding of one editor sends after it, in order, as it makes one edit ' +
  'after another, and json is what an editor reads at the end. schema: the marks in the order the editor stores them, ' +
  'and the attributes of each mark and node with the value given to one that is left out ' +
  '(null: left out).';

/**
 * The identifiers of the clients that write the documents. Whoever reads, or
 * holds what others wrote, is another: a document that is handed its own
 * identifier's writing takes a new one at random.
 */
const WRITER = 1;
const HOLDER = 8;
const READER = 9;

export interface YdocVector {
  name: string;
  update: string;
  json: DocNode | null;
}

export interface BodyVector extends YdocVector {
  body: DocNode;
}

export interface YdocVectors {
  description: string;
  bodyVersion: number;
  field: string;
  schema: {
    marks: { name: string; attrs: Record<string, unknown> }[];
    nodes: Record<string, Record<string, unknown>>;
  };
  documents: YdocVector[];
  bodies: BodyVector[];
  readings: YdocVector[];
  refused: YdocVector[];
  repairs: YdocVector[];
  shortened: YdocVector[];
  session: { start: string; updates: string[]; json: DocNode | null };
}

interface BodyCase {
  name: string;
  body?: unknown;
  expect: string;
}

const base64 = (bytes: Uint8Array): string => Buffer.from(bytes).toString('base64');

/** The bytes of an update written in base64. */
export const fromBase64 = (text: string): Uint8Array => new Uint8Array(Buffer.from(text, 'base64'));

function newDoc(client: number): Y.Doc {
  const doc = new Y.Doc();
  doc.clientID = client;
  return doc;
}

/**
 * What an editor reads from a Y document, in the form that is stored. The
 * document is the reader's own copy, and reading changes it: what does not
 * fit the schema is deleted from it, as the binding of every editor does.
 */
export function readStored(doc: Y.Doc, schema: Schema): DocNode | null {
  const root = yXmlFragmentToProseMirrorRootNode(doc.getXmlFragment(FIELD), schema);
  if (root.childCount === 0) return null;
  const stored = toStoredDoc(root.toJSON() as DocNode);
  return isEmptyDoc(stored) ? null : stored;
}

/** A reader's copy of the Y document an update holds. */
export function readerOf(update: Uint8Array): Y.Doc {
  const doc = newDoc(READER);
  Y.applyUpdate(doc, update);
  return doc;
}

const sameBytes = (one: Uint8Array, other: Uint8Array): boolean =>
  one.length === other.length && one.every((byte, index) => byte === other[index]);

/** What a case is filed under, which says what an editor does with it. */
type Filed = 'kept' | 'refused' | 'repaired' | 'shortened';

/**
 * The vector of a Y document: its update and what an editor makes of it.
 * Fails if the editor does not do what the case is filed under: delete from
 * what is repaired or shortened and from nothing else, and read a body from
 * everything but what is refused or shortened.
 */
function vectorOf(name: string, written: Y.Doc, schema: Schema, filed: Filed): YdocVector {
  const update = Y.encodeStateAsUpdate(written);
  const reader = readerOf(update);
  const json = readStored(reader, schema);
  const after = Y.encodeStateAsUpdate(reader);
  const deleted = !sameBytes(update, after);
  if (deleted !== (filed === 'repaired' || filed === 'shortened')) {
    throw new Error(
      `${name}: the editor ${deleted ? 'deleted something' : 'deleted nothing'} when it read`,
    );
  }
  const again = readStored(reader, schema);
  if (
    JSON.stringify(again) !== JSON.stringify(json) ||
    !sameBytes(after, Y.encodeStateAsUpdate(reader))
  )
    throw new Error(`${name}: reading a second time changed what was read`);
  const problems = json === null ? [] : validateDoc(json);
  const noBody = filed === 'refused' || filed === 'shortened';
  if (noBody ? problems.length === 0 : problems.length > 0) {
    throw new Error(
      noBody
        ? `${name}: what the editor reads is a body`
        : `${name}: what the editor reads is not a body: ${problems.join('; ')}`,
    );
  }
  return { name, update: base64(update), json };
}

function rawDoc(root: RawNode[], client = WRITER): Y.Doc {
  const doc = newDoc(client);
  doc.getXmlFragment(FIELD).insert(0, root.map(rawType));
  return doc;
}

/** A copy of a Y document for another client to edit. */
function fork(base: Y.Doc, client: number): Y.Doc {
  const doc = newDoc(client);
  Y.applyUpdate(doc, Y.encodeStateAsUpdate(base));
  return doc;
}

/** The base with the edits of two clients, each made without the other's. */
function merged(
  base: Y.Doc,
  first: (root: Y.XmlFragment) => void,
  second: (root: Y.XmlFragment) => void,
  schema: Schema,
  name: string,
): Y.Doc {
  vectorOf(`${name} (before the edits)`, base, schema, 'kept');
  const result = fork(base, HOLDER);
  [first, second].forEach((edit, index) => {
    const side = fork(base, WRITER + 1 + index);
    edit(side.getXmlFragment(FIELD));
    // Each edit alone leaves a body, which every editor keeps as it is.
    vectorOf(`${name} (edit ${index + 1} alone)`, side, schema, 'kept');
    Y.applyUpdate(result, Y.encodeStateAsUpdate(side));
  });
  return result;
}

const childOf = (parent: Y.XmlFragment, index: number): Y.XmlFragment => {
  const child = parent.get(index);
  if (!(child instanceof Y.XmlFragment)) throw new Error(`no element at ${index}`);
  return child;
};

/** Shared content that two valid edits leave when they are merged. */
function mergedRepairs(schema: Schema): YdocVector[] {
  const text = (insert: string): RawNode => ({ text: [{ insert }] });
  const paragraph = (insert: string): RawNode => ({
    element: 'paragraph',
    children: [text(insert)],
  });
  const entry = (...children: RawNode[]): RawNode => ({ element: 'listItem', children });
  const made: [string, Y.Doc][] = [];

  const emptied = 'two people each delete one of the two entries of a list';
  made.push([
    emptied,
    merged(
      rawDoc([
        paragraph('a'),
        { element: 'bulletList', children: [entry(paragraph('one')), entry(paragraph('two'))] },
      ]),
      (root) => {
        childOf(root, 1).delete(0, 1);
      },
      (root) => {
        childOf(root, 1).delete(1, 1);
      },
      schema,
      emptied,
    ),
  ]);

  const headless =
    'one deletes the first paragraph of an entry, the other puts a quote in place of the second';
  made.push([
    headless,
    merged(
      rawDoc([
        {
          element: 'bulletList',
          children: [entry(paragraph('one'), paragraph('more')), entry(paragraph('two'))],
        },
      ]),
      (root) => {
        childOf(childOf(root, 0), 0).delete(0, 1);
      },
      (root) => {
        const first = childOf(childOf(root, 0), 0);
        first.delete(1, 1);
        first.insert(1, [rawType({ element: 'blockquote', children: [paragraph('q')] })]);
      },
      schema,
      headless,
    ),
  ]);

  return made.map(([name, doc]) => vectorOf(name, doc, schema, 'repaired'));
}

/** The vectors of merged cases, filed as what an editor does with what they leave. */
function mergedVectors(cases: MergedCase[], schema: Schema, filed: Filed): YdocVector[] {
  return cases.map((one) =>
    vectorOf(
      one.name,
      merged(rawDoc(one.base), one.first, one.second, schema, one.name),
      schema,
      filed,
    ),
  );
}

/** The attributes a kind of node or mark has, with the value given to one left out. */
function attributesOf(
  kind: string,
  attrs: Record<string, { default?: unknown }> | undefined,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(attrs ?? {}).map(([name, spec]) => {
      // Without a default a node that lacks the attribute could not be made
      // at all, and an editor would delete it: another rule than the one here.
      if (!('default' in spec)) throw new Error(`${kind}.${name} has no default`);
      return [name, spec.default ?? null];
    }),
  );
}

/** The Y document the binding makes of a body. */
function docOfBody(schema: Schema, body: unknown): Y.Doc {
  const doc = newDoc(WRITER);
  prosemirrorToYXmlFragment(Node.fromJSON(schema, body), doc.getXmlFragment(FIELD));
  return doc;
}

/**
 * One editor's session: the document the server opens, and every update the
 * binding sends as the editor's document becomes each state in turn.
 */
function session(schema: Schema): YdocVectors['session'] {
  const [opened, ...states] = SESSION;
  const start = Y.encodeStateAsUpdate(docOfBody(schema, opened));
  const editor = newDoc(WRITER + 1);
  Y.applyUpdate(editor, start);
  const updates: string[] = [];
  editor.on('update', (update: Uint8Array) => updates.push(base64(update)));
  for (const state of states)
    prosemirrorToYXmlFragment(Node.fromJSON(schema, state), editor.getXmlFragment(FIELD));
  const json = readStored(readerOf(Y.encodeStateAsUpdate(editor)), schema);
  const last = toStoredDoc(Node.fromJSON(schema, states.at(-1)).toJSON() as DocNode);
  if (JSON.stringify(json) !== JSON.stringify(last))
    throw new Error('the session does not end in its last state');
  return { start: base64(start), updates, json };
}

/** Everything the file holds, from the editor's schema and the accepted bodies. */
export function buildVectors(schema: Schema, bodyCases: BodyCase[]): YdocVectors {
  // That an editor keeps every one of them as it is, is checked as it is read.
  const documents = bodyCases
    .filter((one) => one.expect === 'accept' && one.body !== undefined)
    .map((one) => vectorOf(one.name, docOfBody(schema, one.body), schema, 'kept'));
  const bodies = BODIES.map((one) => {
    const problems = validateDoc(one.body);
    if (problems.length > 0) throw new Error(`${one.name}: no body: ${problems.join('; ')}`);
    return { ...vectorOf(one.name, docOfBody(schema, one.body), schema, 'kept'), body: one.body };
  });
  const raw = (cases: RawCase[], filed: Filed): YdocVector[] =>
    cases.map((one) => vectorOf(one.name, rawDoc(one.root), schema, filed));
  return {
    description: DESCRIPTION,
    bodyVersion: BODY_VERSION,
    field: FIELD,
    schema: {
      marks: Object.entries(schema.marks).map(([name, type]) => ({
        name,
        attrs: attributesOf(name, type.spec.attrs),
      })),
      nodes: Object.fromEntries(
        Object.entries(schema.nodes)
          .map(([name, type]) => [name, attributesOf(name, type.spec.attrs)] as const)
          .sort(([one], [other]) => (one < other ? -1 : 1)),
      ),
    },
    documents,
    bodies,
    readings: [...raw(READINGS, 'kept'), ...raw(DETAILS_READINGS, 'kept')],
    refused: [
      ...raw(REFUSED, 'refused'),
      ...raw(TABLE_REFUSED, 'refused'),
      ...mergedVectors(TABLE_MERGED_REFUSED, schema, 'refused'),
      ...raw(DETAILS_REFUSED, 'refused'),
      ...mergedVectors(DETAILS_MERGED_REFUSED, schema, 'refused'),
    ],
    repairs: [
      ...mergedRepairs(schema),
      ...raw(REPAIRS, 'repaired'),
      ...raw(TABLE_REPAIRS, 'repaired'),
      ...raw(DETAILS_REPAIRS, 'repaired'),
      ...mergedVectors(DETAILS_MERGED_REPAIRS, schema, 'repaired'),
    ],
    shortened: [
      ...raw(TABLE_SHORTENED, 'shortened'),
      ...mergedVectors(TABLE_MERGED_SHORTENED, schema, 'shortened'),
    ],
    session: session(schema),
  };
}

/** The accepted and refused bodies both sides are held to. */
export function readBodyCases(): BodyCase[] {
  return (JSON.parse(readFileSync(BODY_VECTORS_FILE, 'utf8')) as { cases: BodyCase[] }).cases;
}

/** The text of the file for what it holds. */
export const serialize = (vectors: YdocVectors): string => `${JSON.stringify(vectors, null, 2)}\n`;

/**
 * The schema of the product's editor. Its extensions reach the app's texts
 * through an alias only the build knows, so they are loaded as the build
 * loads them.
 */
async function productSchema(): Promise<Schema> {
  const { createServer } = await import('vite');
  const root = path.resolve(import.meta.dirname, '../..');
  const server = await createServer({
    root,
    configFile: path.join(root, 'vite.config.ts'),
    server: { middlewareMode: true, hmr: false, ws: false },
    appType: 'custom',
    logLevel: 'error',
  });
  try {
    const editor = (await server.ssrLoadModule('/src/lib/editor/extensions.ts')) as {
      bodyExtensions: () => unknown[];
    };
    const core = (await server.ssrLoadModule('@tiptap/core')) as {
      getSchema: (extensions: unknown[]) => Schema;
    };
    return core.getSchema(editor.bodyExtensions());
  } finally {
    await server.close();
  }
}

if (import.meta.main) {
  const written = serialize(buildVectors(await productSchema(), readBodyCases()));
  if (process.argv.includes('--check')) {
    let committed = '';
    try {
      committed = readFileSync(YDOC_VECTORS_FILE, 'utf8');
    } catch {
      // No file is a stale file.
    }
    if (committed !== written) {
      console.error('contracts/ydoc-vectors.json is not what tools/ydoc/vectors.ts writes');
      process.exit(1);
    }
    console.log('contracts/ydoc-vectors.json: up to date');
  } else {
    writeFileSync(YDOC_VECTORS_FILE, written);
    console.log(`wrote ${path.relative(process.cwd(), YDOC_VECTORS_FILE)}`);
  }
}
