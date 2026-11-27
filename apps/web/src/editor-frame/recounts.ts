/**
 * What a save counts otherwise than the document had it: its pages, and its
 * controls of each kind (viewer-frame/hwp-controls.ts). rhwp may write a
 * document it lays out on 157 pages as one it reads back on 159, or a
 * control it reads as one it then reads as none, and its report of the save
 * says nothing of either. The writer is asked about these as about what the
 * report names.
 *
 * The pages are counted in the viewer's worker, both: the file as it was
 * opened, and that file saved as rhwp saves it and read back. The editor
 * lays its document out otherwise (it measures text in the frame, and lays
 * out again only what an edit moves), and its count is no count of the same
 * kind as a file's read back. The controls do not depend on the layout: the
 * document in the editor is counted against its saved file read back.
 */
import {
  CONTROL_COUNT_LIMIT,
  RECOUNT_LIMIT,
  type Recount,
} from '$lib/file-viewer/editor-protocol.ts';
import { PAGE_LIMIT } from '$lib/file-viewer/protocol.ts';

import type { ControlTally } from '../viewer-frame/hwp-controls.ts';

/** The pages the file opened lays out on, and the pages it lays out on saved and read back. */
export type Resaved = readonly [pages: number, saved: number];

/**
 * The counts that differ: the pages first, where the file opened, saved, reads
 * back on other pages, then each kind of control the saved file (`after`)
 * counts otherwise than the document in the editor (`before`), by its id.
 * What was not counted (undefined) is not compared.
 */
export function recountsOf(
  pages: Resaved | undefined,
  before: ControlTally | undefined,
  after: ControlTally | undefined,
): Recount[] {
  const recounts: Recount[] = [];
  const page = (count: number): number => Math.min(count, PAGE_LIMIT);
  // A file on no page is no file read back: its pages are not a count to ask about.
  if (pages && Math.min(...pages) >= 1 && page(pages[0]) !== page(pages[1])) {
    recounts.push({ of: 'pages', before: page(pages[0]), after: page(pages[1]) });
  }
  if (!before || !after) return recounts;
  const was = new Map(before);
  const is = new Map(after);
  const ids = [...new Set([...was.keys(), ...is.keys()])].sort();
  const controls = (count = 0): number => Math.min(count, CONTROL_COUNT_LIMIT);
  for (const id of ids) {
    const [one, other] = [controls(was.get(id)), controls(is.get(id))];
    if (one !== other) recounts.push({ of: 'controls', id, before: one, after: other });
  }
  return recounts.slice(0, RECOUNT_LIMIT);
}

const same = (one: Recount, other: Recount): boolean =>
  one.of === other.of &&
  one.before === other.before &&
  one.after === other.after &&
  (one.of === 'pages' || (other.of === 'controls' && one.id === other.id));

/**
 * The counts not among those the writer saved a version with already: the same
 * difference again (the editor's document has the place of its page numbers
 * twice still, and the file once) is not asked about again; another is.
 */
export function notAccepted(recounts: Recount[], accepted: readonly Recount[]): Recount[] {
  return recounts.filter((recount) => !accepted.some((one) => same(one, recount)));
}
