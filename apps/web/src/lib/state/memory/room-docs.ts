/**
 * The Y documents of the memory rooms (rooms.ts) and the bodies they hold:
 * made from a body the same way in every tab, read back as it is stored.
 */
import { getSchema } from '@tiptap/core';
import { prosemirrorJSONToYXmlFragment } from '@tiptap/y-tiptap';
import * as Y from 'yjs';

import { BODY_FIELD } from '../../editor/body-editor.ts';
import { bodyExtensions } from '../../editor/extensions.ts';
import { bodyOfDoc, readBodyOfDoc } from '../../editor/room-body.ts';
import { sameDocNode } from '../../editor/same-doc.ts';
import { emptyDoc, type DocNode } from '../../editor/schema.ts';

import type { StoredCard } from './world.ts';

/** The client of a document made from a body: the same in every tab, so that two makings are one. */
const MAKER = 1;

/** The editor's schema, which a body is read by. */
export const bodySchema = getSchema(bodyExtensions());

/** The document of a body, made the same way wherever it is made. */
export function documentOf(body: DocNode | null): Y.Doc {
  const doc = new Y.Doc();
  doc.clientID = MAKER;
  prosemirrorJSONToYXmlFragment(bodySchema, body ?? emptyDoc(), doc.getXmlFragment(BODY_FIELD));
  // Whatever is written next is somebody's: never the maker's again.
  doc.clientID = Math.floor(Math.random() * 0x7fff_ffff) + 2;
  return doc;
}

/** The body a document holds, as it would be stored; undefined when it is not one. */
export const bodyOf = bodyOfDoc;

/** Reads and validates a document's body in one pass, naming the refusal reason on failure. */
export const readBodyOf = readBodyOfDoc;

/** Whether a body read from a document is `stored`, read as a document made from it would hold it. */
export function sameBody(read: DocNode | null, stored: DocNode | null): boolean {
  const made = documentOf(stored);
  const same = sameDocNode(read, bodyOf(made) ?? null);
  made.destroy();
  return same;
}

/** What `doc` has that a holder of `stateVector` lacks; undefined when that is no state vector. */
export function lackedBy(doc: Y.Doc, stateVector: Uint8Array): Uint8Array | undefined {
  try {
    return Y.encodeStateAsUpdate(doc, stateVector);
  } catch {
    return undefined;
  }
}

/** A short sum of a text (FNV-1a), to tell apart bodies a document was made from. */
function sum(text: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash = Math.imul(hash ^ text.charCodeAt(index), 0x01000193) >>> 0;
  }
  return hash.toString(36);
}

/** What makes two tabs' rooms one: the card, its making, the epoch and the body the document came from. */
export function tabKey(card: StoredCard | undefined, cardId: string, epoch: number): string {
  const made = sum(JSON.stringify(card?.body ?? null));
  return `${card?.containerId ?? ''} ${cardId} ${String(card?.createdAt ?? 0)} ${String(epoch)} ${made}`;
}
