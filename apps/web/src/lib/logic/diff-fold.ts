/**
 * The merged blocks of a comparison (body-diff.ts) in stretches to show and
 * to fold away, for the view of a comparison. Apart from the comparison, so
 * that the view does not bring it along: that is loaded when it is needed.
 */
import type { DiffNode } from './body-nodes.ts';

/** A stretch of the merged blocks: shown, or folded away for being the same in both. */
export interface DiffSegment {
  folded: boolean;
  blocks: DiffNode[];
}

/**
 * The merged blocks in stretches to show and to fold: of a run of blocks the
 * same in both, `context` stay shown next to each change around it, and the
 * rest is folded when it is at least `least` blocks.
 */
export function foldSame(blocks: DiffNode[], context = 1, least = 3): DiffSegment[] {
  const segments: DiffSegment[] = [];
  const add = (folded: boolean, part: DiffNode[]): void => {
    if (part.length === 0) return;
    const last = segments.at(-1);
    if (!folded && last?.folded === false) last.blocks.push(...part);
    else segments.push({ folded, blocks: [...part] });
  };
  let start = 0;
  while (start < blocks.length) {
    const block = blocks[start];
    if (block?.diff !== undefined) {
      add(false, [block]);
      start += 1;
      continue;
    }
    let end = start;
    while (end < blocks.length && blocks[end]?.diff === undefined) end += 1;
    const before = start === 0 ? 0 : context;
    const after = end === blocks.length ? 0 : context;
    if (end - start - before - after >= least) {
      add(false, blocks.slice(start, start + before));
      add(true, blocks.slice(start + before, end - after));
      add(false, blocks.slice(end - after, end));
    } else add(false, blocks.slice(start, end));
    start = end;
  }
  return segments;
}
