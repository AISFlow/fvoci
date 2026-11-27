/**
 * What the card store is built on besides the backend: the shapes it shares
 * with the screens, and what became of a card whose room let the reader go.
 */
import { isBackendError } from './backend.ts';

/** How a change of a comment ended: stored, refused for a newer version, or failed (and told). */
export type CommentEdit = 'saved' | 'conflict' | 'failed';

/**
 * How long after its room was joined again by itself (`afterPutOut` said
 * `there`) a card's room is not joined again by itself: a server that lets
 * the reader in and puts them out again is not asked in a loop.
 */
export const REJOIN_SPACING = 10_000;

/**
 * What became of a card whose room let the reader go as it does for a card
 * that is not there (`not_found`), as reading the card again says:
 *
 * - `there`: it is read. It was restored since it was deleted, or is the
 *   reader's to read again.
 * - `gone`: 403 or 404. It was deleted, or is no longer the reader's to read
 *   (taken off its board, out of its workspace).
 * - `ended`: 401. The reader's session ended, or the organization takes no
 *   password any more.
 * - `unknown`: the reading failed otherwise.
 */
export type PutOut =
  | { readonly found: 'there' | 'gone' | 'unknown' }
  | { readonly found: 'ended'; readonly error: unknown };

/** Reads the card with `read` after its room let the reader go: what became of it. */
export async function afterPutOut(read: () => Promise<unknown>): Promise<PutOut> {
  try {
    await read();
    return { found: 'there' };
  } catch (error) {
    if (!isBackendError(error)) return { found: 'unknown' };
    if (error.status === 401) return { found: 'ended', error };
    return { found: error.status === 403 || error.status === 404 ? 'gone' : 'unknown' };
  }
}
