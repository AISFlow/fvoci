/**
 * The vectors of a caret (`contracts/caret-vectors.json`), which the
 * server's reading and writing of carets is held against
 * (`crates/server/src/collab/caret.rs`): that the file is what the tool
 * writes with the editor's libraries, that those libraries read what the
 * server tells as the caret it is, and that the rooms of the backend in
 * memory (`state/memory/caret-frame.ts`) read and write as the server does.
 */
import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';
import { applyAwarenessUpdate, Awareness } from 'y-protocols/awareness';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import {
  CARET_BYTES,
  CARET_FIELD,
  caretSaid,
  caretState,
  caretTold,
} from '$lib/state/memory/caret-frame.ts';

import {
  buildCaretVectors,
  CARET_VECTORS_FILE,
  serializeCarets,
  type CaretVectors,
} from '../../tools/ydoc/carets.ts';

const committed = readFileSync(CARET_VECTORS_FILE, 'utf8');
const vectors = JSON.parse(committed) as CaretVectors;
const bytes = (text: string): Uint8Array => new Uint8Array(Buffer.from(text, 'base64'));

describe('the vectors of a caret', () => {
  it('are what the tool writes', () => {
    expect(serializeCarets(buildCaretVectors())).toBe(committed);
  });

  it("are read by the editor's library as the server's word of the caret", () => {
    for (const each of vectors.cases) {
      const awareness = new Awareness(new Y.Doc());
      try {
        // A caret the reader holds already, to see one that is gone go.
        applyAwarenessUpdate(awareness, caretTold(each.number, each.clock - 1, '{}'), 'test');
        applyAwarenessUpdate(awareness, bytes(each.told), 'test');
        const held = awareness.getStates().get(each.number);
        if (each.state === null) expect(held, each.name).toBeUndefined();
        else {
          expect(held, each.name).toEqual(JSON.parse(each.state));
          expect((held as { user: unknown }).user, each.name).toEqual({ id: each.userId });
        }
      } finally {
        awareness.destroy();
      }
    }
  });

  it('are read and written by the rooms in memory as by the server', () => {
    expect(CARET_FIELD).toBe(BODY_FIELD);
    expect(vectors.cases.length).toBeGreaterThanOrEqual(8);
    for (const each of vectors.cases) {
      const said = caretSaid(bytes(each.said));
      expect(said, each.name).not.toBeUndefined();
      const state = said ? caretState(each.userId, said) : null;
      expect(state, each.name).toBe(each.state);
      expect(caretTold(each.number, each.clock, state), each.name).toEqual(bytes(each.told));
      expect(bytes(each.said).length, each.name).toBeLessThanOrEqual(CARET_BYTES);
    }
    expect(vectors.refused.length).toBeGreaterThanOrEqual(13);
    for (const each of vectors.refused) {
      expect(caretSaid(bytes(each.said)), each.name).toBeUndefined();
    }
  });

  it('name places the library finds again in the document', () => {
    // The first case is a selection in a line of a document made as the tool makes it.
    const doc = new Y.Doc();
    doc.clientID = 11;
    const fragment = doc.getXmlFragment('default');
    const paragraph = new Y.XmlElement('paragraph');
    const text = new Y.XmlText('Hello caret');
    fragment.insert(0, [paragraph, new Y.XmlElement('paragraph')]);
    paragraph.insert(0, [text]);
    const [first] = vectors.cases;
    const state = JSON.parse(first?.state ?? 'null') as {
      cursor: { anchor: unknown; head: unknown };
    };
    const at = (place: unknown): number | undefined =>
      Y.createAbsolutePositionFromRelativePosition(Y.createRelativePositionFromJSON(place), doc)
        ?.index;
    expect([at(state.cursor.anchor), at(state.cursor.head)]).toEqual([2, 7]);
  });
});
