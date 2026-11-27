/**
 * Writes `contracts/caret-vectors.json`: carets as the editor's libraries say
 * them (y-protocols' awareness, Yjs's relative positions), and what the
 * server tells the others of each, for the server's reading and writing of
 * them (`crates/server/src/collab/caret.rs`) to be held against.
 *
 *   node tools/ydoc/carets.ts           writes the file
 *   node tools/ydoc/carets.ts --check   fails when the file is not what would be written
 *
 * - `cases`: an awareness update a client sends (`said`), and the update the
 *   server sends on (`told`) for the user, number and clock given: the state
 *   names the server's user and the two places, and nothing else the client
 *   wrote; `state` is `null` where the client shows no caret.
 * - `refused`: updates the server sends nothing on for.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import { Awareness, encodeAwarenessUpdate } from 'y-protocols/awareness';
import * as Y from 'yjs';

const CONTRACTS = path.resolve(import.meta.dirname, '../../../../contracts');
export const CARET_VECTORS_FILE = path.join(CONTRACTS, 'caret-vectors.json');

const DESCRIPTION =
  'Carets as awareness updates of y-protocols, written by apps/web/tools/ydoc/carets.ts with ' +
  "the editor's own libraries; the server's tests (crates/server/src/collab/caret.rs) and the " +
  "web app's read it. Do not edit by hand. said is what a client sends and told what the server " +
  'sends on for it, both in base64; state is the JSON the server tells, null for no caret.';

/** The field of a body's Y document the editor is bound to. */
const FIELD = 'default';

export interface CaretCase {
  name: string;
  said: string;
  /** Whose connection sent it, as the server knows. */
  userId: string;
  /** The number and the clock the server tells it under. */
  number: number;
  clock: number;
  state: string | null;
  told: string;
}

export interface CaretVectors {
  description: string;
  cases: CaretCase[];
  refused: { name: string; said: string }[];
}

const toBase64 = (bytes: Uint8Array): string => Buffer.from(bytes).toString('base64');

/** The awareness update of one state, as a client whose document's client is `client` sends it. */
function saidBy(client: number, states: [number, unknown][], clock = 4): Uint8Array {
  const doc = new Y.Doc();
  doc.clientID = client;
  const awareness = new Awareness(doc);
  try {
    for (const [each] of states) awareness.meta.set(each, { clock, lastUpdated: 0 });
    const held = new Map(states.filter(([, state]) => state !== null)) as Map<
      number,
      Record<string, unknown>
    >;
    return encodeAwarenessUpdate(
      awareness,
      states.map(([each]) => each),
      held,
    );
  } finally {
    awareness.destroy();
    doc.destroy();
  }
}

/** A place as the server writes it: what Yjs's JSON of it has, without what is null. */
function placeOf(position: Y.RelativePosition): Record<string, unknown> {
  const json = JSON.parse(JSON.stringify(position)) as Record<string, unknown>;
  return Object.fromEntries(Object.entries(json).filter(([, value]) => value !== null));
}

interface Places {
  anchor: Y.RelativePosition;
  head: Y.RelativePosition;
}

/** A document of one paragraph and one empty one, and places in it. */
function places(): { line: Places; start: Places; empty: Places } {
  const doc = new Y.Doc();
  doc.clientID = 11;
  const fragment = doc.getXmlFragment(FIELD);
  const start = Y.createRelativePositionFromTypeIndex(fragment, 0);
  const paragraph = new Y.XmlElement('paragraph');
  const text = new Y.XmlText('Hello caret');
  const second = new Y.XmlElement('paragraph');
  fragment.insert(0, [paragraph, second]);
  paragraph.insert(0, [text]);
  const places = {
    line: {
      anchor: Y.createRelativePositionFromTypeIndex(text, 2),
      head: Y.createRelativePositionFromTypeIndex(text, 7, -1),
    },
    start: { anchor: start, head: start },
    empty: {
      anchor: Y.createRelativePositionFromTypeIndex(second, 0),
      head: Y.createRelativePositionFromTypeIndex(second, 0),
    },
  };
  doc.destroy();
  return places;
}

const USER = 'u7k3mpq1x9fjzr5d2a8wbn4yhc';

function caseOf(
  name: string,
  state: Record<string, unknown> | null,
  cursor: Places | undefined,
  number: number,
  clock: number,
): CaretCase {
  const told = cursor
    ? { user: { id: USER }, cursor: { anchor: placeOf(cursor.anchor), head: placeOf(cursor.head) } }
    : null;
  return {
    name,
    said: toBase64(saidBy(424_242, [[424_242, state]])),
    userId: USER,
    number,
    clock,
    state: told === null ? null : JSON.stringify(told),
    told: toBase64(saidBy(1, [[number, told]], clock)),
  };
}

export function buildCaretVectors(): CaretVectors {
  const { line, start, empty } = places();
  /** What a client says of itself beside its caret: none of it is the server's to send on. */
  const somebodyElse = { id: 'somebody-else', name: 'Mallory', color: '#ff0000' };
  const sound = saidBy(424_242, [[424_242, { user: {}, cursor: line }]]);
  const withTail = new Uint8Array(sound.length + 1);
  withTail.set(sound);
  const refuse = (name: string, said: Uint8Array): { name: string; said: string } => ({
    name,
    said: toBase64(said),
  });
  const huge = { ...placeOf(line.anchor), item: { client: 2 ** 53, clock: 0 } };
  return {
    description: DESCRIPTION,
    cases: [
      caseOf('a selection in a line of text', { user: {}, cursor: line }, line, 7, 1),
      caseOf(
        'a caret that says whose it is: the server says whose',
        { user: somebodyElse, cursor: line, more: [1, 2, 3] },
        line,
        300,
        129,
      ),
      caseOf('a caret at the start of a body', { user: {}, cursor: start }, start, 8, 2),
      caseOf('a caret in an empty paragraph', { user: {}, cursor: empty }, empty, 9, 3),
      caseOf(
        'a caret beside what no reader would keep: half of a pair of surrogates in a name',
        { user: { name: 'cut \ud83d' }, cursor: line },
        line,
        4_294_967_297,
        1,
      ),
      caseOf('a state that names no caret', { user: somebodyElse, cursor: null }, undefined, 7, 2),
      caseOf('a state without a caret', { user: {} }, undefined, 7, 2),
      caseOf('a state that is gone', null, undefined, 16_384, 2_097_152),
    ],
    refused: [
      refuse(
        'two states in one update',
        saidBy(424_242, [
          [424_242, { cursor: line }],
          [7, { cursor: line }],
        ]),
      ),
      refuse('no state at all', saidBy(424_242, [])),
      refuse('a byte after the state', withTail),
      refuse('a state cut short', sound.subarray(0, sound.length - 3)),
      refuse(
        'a caret with one end',
        saidBy(424_242, [[424_242, { cursor: { anchor: line.anchor } }]]),
      ),
      refuse(
        'a place with something no place has',
        saidBy(424_242, [
          [
            424_242,
            { cursor: { anchor: { ...placeOf(line.anchor), name: 'x' }, head: line.head } },
          ],
        ]),
      ),
      refuse(
        'a place of a client no browser counts to',
        saidBy(424_242, [[424_242, { cursor: { anchor: huge, head: line.head } }]]),
      ),
      refuse(
        'a place that names nothing',
        saidBy(424_242, [[424_242, { cursor: { anchor: { assoc: 0 }, head: line.head } }]]),
      ),
      refuse(
        "a place in a root type that is not the body's",
        saidBy(424_242, [
          [424_242, { cursor: { anchor: { tname: 'another', assoc: 0 }, head: line.head } }],
        ]),
      ),
      refuse('a caret that is no two places', saidBy(424_242, [[424_242, { cursor: 'here' }]])),
      refuse(
        'a caret as a list of two places',
        saidBy(424_242, [[424_242, { cursor: [line.anchor, line.head] }]]),
      ),
      refuse(
        'a place as a list',
        saidBy(424_242, [
          [424_242, { cursor: { anchor: [null, null, null, 0], head: line.head } }],
        ]),
      ),
      refuse('a state that is no object', saidBy(424_242, [[424_242, [1, 2]]])),
    ],
  };
}

export const serializeCarets = (vectors: CaretVectors): string =>
  `${JSON.stringify(vectors, null, 2)}\n`;

if (import.meta.main) {
  const written = serializeCarets(buildCaretVectors());
  if (process.argv.includes('--check')) {
    let committed = '';
    try {
      committed = readFileSync(CARET_VECTORS_FILE, 'utf8');
    } catch {
      // No file is a stale file.
    }
    if (committed !== written) {
      console.error('contracts/caret-vectors.json is not what tools/ydoc/carets.ts writes');
      process.exit(1);
    }
    console.log('contracts/caret-vectors.json: up to date');
  } else {
    writeFileSync(CARET_VECTORS_FILE, written);
    console.log(`wrote ${path.relative(process.cwd(), CARET_VECTORS_FILE)}`);
  }
}
