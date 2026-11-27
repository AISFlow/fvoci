/**
 * The body a card's or a page's document in its room holds now, as it would
 * be stored: what the reader and the others typed, which the stored body has
 * only once whoever holds the room writes it. Fetched on its own where it is
 * needed (the comparison of a version with the body now), so that what reads
 * Yjs's document comes with the editor and no sooner.
 */
import { yXmlFragmentToProsemirrorJSON } from '@tiptap/y-tiptap';
import type { Doc } from 'yjs';

import { BODY_FIELD } from './body-editor.ts';
import {
  isEmptyDoc,
  TABLE_TOO_LARGE_PROBLEM,
  toStoredDoc,
  validateDoc,
  type DocNode,
} from './schema.ts';

/** Result of reading and validating a room's Y document as a stored body. */
export type ReadBody =
  { ok: true; body: DocNode | null } | { ok: false; reason: 'table_too_large' | 'update_refused' };

/**
 * Reads and validates a room's Y document in a single pass, distinguishing an
 * oversized table (`table_too_large`) from other invalid documents (`update_refused`).
 */
export function readBodyOfDoc(doc: Doc): ReadBody {
  try {
    const fragment = doc.getXmlFragment(BODY_FIELD);
    if (fragment.length === 0) return { ok: true, body: null };
    const stored = toStoredDoc(yXmlFragmentToProsemirrorJSON(fragment) as DocNode);
    const problems = validateDoc(stored);
    if (problems.length > 0) {
      const tableOnly = problems.every((problem) =>
        problem.endsWith(`: ${TABLE_TOO_LARGE_PROBLEM}`),
      );
      return { ok: false, reason: tableOnly ? 'table_too_large' : 'update_refused' };
    }
    return { ok: true, body: isEmptyDoc(stored) ? null : stored };
  } catch {
    return { ok: false, reason: 'update_refused' };
  }
}

/**
 * The body a document holds, as it would be stored; undefined when it is not
 * one. A document with nothing in it (a merge can leave one: two people each
 * deleting a column of the only table) has no body, as the server reads it.
 */
export function bodyOfDoc(doc: Doc): DocNode | null | undefined {
  const read = readBodyOfDoc(doc);
  return read.ok ? read.body : undefined;
}
