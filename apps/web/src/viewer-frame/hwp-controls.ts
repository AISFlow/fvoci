/**
 * How many controls of each kind a document holds, as rhwp lists them
 * (`getControls`): the kinds Hangul calls 조판 부호, such as a table (`tbl`),
 * a picture or drawing (`gso`), a header (`head`) or the place of the page
 * numbers (`pgnp`), each by the id the format gives it.
 *
 * The editor's frame compares the tally of the document in the editor with
 * that of its saved file read back (editor-frame/recounts.ts): rhwp may write
 * a control it reads as one it then reads otherwise, or not at all, and its
 * report does not say so.
 */
import { CONTROL_ID } from '$lib/file-viewer/editor-protocol.ts';

/** The kinds and how many of each, ordered by id. */
export type ControlTally = readonly (readonly [id: string, count: number])[];

/** The kind of a control whose id is not one: counted together. */
const UNNAMED = '';

/** The tally of rhwp's list of controls; throws on a list that is not one. */
export function tallyControls(list: unknown): ControlTally {
  if (!Array.isArray(list)) throw new Error('not a list of controls');
  const counts = new Map<string, number>();
  for (const control of list) {
    const found: unknown = (control as { ctrlId?: unknown } | null)?.ctrlId;
    if (typeof found !== 'string') throw new Error('a control without its id');
    const id = CONTROL_ID.test(found) ? found : UNNAMED;
    counts.set(id, (counts.get(id) ?? 0) + 1);
  }
  return [...counts].sort(([one], [other]) => (one < other ? -1 : one > other ? 1 : 0));
}
