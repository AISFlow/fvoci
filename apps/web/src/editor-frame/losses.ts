/**
 * rhwp's report of what an export loses, as the protocol carries it
 * (`$lib/file-viewer/editor-protocol.ts`): how many losses, and the first of them.
 *
 * The frame reads the report as strictly as the app reads the message made of
 * it: a report that is not in the shape rhwp 0.9.0 writes, or that names a
 * loss this version of the app has no words for, is no report, and bytes are
 * not handed over without one (the export fails, and says so). Were it left
 * to the app's reader, the message would be dropped there and the writer left
 * waiting.
 */
import {
  LOSS_CODES,
  LOSS_COUNT_LIMIT,
  LOSS_LIST_LIMIT,
  LOSS_PATH_LIMIT,
  LOSS_REASONS,
  LOSS_SUBJECTS,
  type Loss,
} from '$lib/file-viewer/editor-protocol.ts';

const isOneOf = <T extends string>(list: readonly T[], value: unknown): value is T =>
  typeof value === 'string' && (list as readonly string[]).includes(value);

function lossOf(loss: unknown): Loss {
  const { code, subject, reason, path } = (loss ?? {}) as Record<string, unknown>;
  if (!isOneOf(LOSS_CODES, code)) throw new Error('a loss of a kind not known');
  if (!isOneOf(LOSS_SUBJECTS, subject)) throw new Error('a loss of a subject not known');
  if (!isOneOf(LOSS_REASONS, reason)) throw new Error('a loss for a reason not known');
  if (typeof path !== 'string') throw new Error('a loss has no path');
  return { code, subject, reason, path: path.slice(0, LOSS_PATH_LIMIT) };
}

/** Throws on a report that is not one, in whole or in any loss it lists. */
export function lossesOf(report: unknown): { count: number; losses: Loss[] } {
  const { schemaVersion, count, losses } = (report ?? {}) as Record<string, unknown>;
  if (schemaVersion !== 1 || !Number.isInteger(count) || !Array.isArray(losses)) {
    throw new Error('the report of losses is not one');
  }
  if (losses.length !== count) throw new Error('the report counts other than it lists');
  if (losses.length > LOSS_COUNT_LIMIT) throw new Error('the report lists too much');
  // Every loss is read, also those past what the message lists.
  const read = losses.map(lossOf);
  return { count: read.length, losses: read.slice(0, LOSS_LIST_LIMIT) };
}
