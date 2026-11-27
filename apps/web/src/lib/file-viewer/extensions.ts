/**
 * The files the viewer is likely to show, by what is known of them before
 * their bytes are read: the extension of their name and their size. A file
 * block shows the name of such a file as a button that opens the viewer;
 * any other offers only its download.
 *
 * The name is what the person who uploaded the file said. Whether the
 * viewer draws the file is still decided by its bytes (`formats.ts`), and
 * one that is not what its name says is offered as a download there. A
 * module without imports but a type: it is in what the first screen loads,
 * and `formats.ts` is not.
 */
import type { FormatId } from './formats.ts';

/** The largest file the viewer opens; a larger one is only offered as a download. */
export const VIEW_LIMIT_BYTES = 50 * 1024 * 1024;

/** The extensions of each kind the viewer draws: adding a kind to `formats.ts` asks for its line. */
const EXTENSIONS: Record<FormatId, readonly string[]> = {
  pdf: ['pdf'],
  docx: ['docx'],
  xlsx: ['xlsx', 'xlsm'],
  pptx: ['pptx'],
  hwpx: ['hwpx'],
  hwp: ['hwp'],
};

const OPENED = new Set(Object.values(EXTENSIONS).flat());

/** Whether the viewer is likely to show a file of this name and size (in bytes; 0 when unknown). */
export function opensInViewer(name: string, size: number): boolean {
  const dot = name.lastIndexOf('.');
  if (dot < 0 || size > VIEW_LIMIT_BYTES) return false;
  return OPENED.has(name.slice(dot + 1).toLowerCase());
}
