/**
 * Where the pictures taken with `FVOCI_SCREENS=1` go: `target/screens/<area>/`
 * at the top of the repository, a folder that is not committed.
 */
import path from 'node:path';

export const SCREENS_ROOT = path.resolve(import.meta.dirname, '../../../target/screens');

/** The folder for the pictures of one area, such as `card` or `kanban`. */
export function screensDir(area: string): string {
  return path.join(SCREENS_ROOT, area);
}
