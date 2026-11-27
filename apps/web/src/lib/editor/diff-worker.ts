/**
 * The comparison of two bodies (logic/body-diff.ts), in a worker of its own:
 * given two bodies, it answers with what changed from the one to the other.
 *
 * It runs apart from the page because the largest bodies take a moment:
 * two bodies of 100,000 nodes that differ all through
 * (tests/logic/body-diff-size.test.ts), during which a page would not move.
 * Built as a file of the app's own (`?worker`), as the highlighter's is.
 */
import { diffBodies, type BodyDiffOptions } from '$lib/logic/body-diff.ts';
import type { BodyNode } from '$lib/logic/body-nodes.ts';

interface Asked {
  id: number;
  before: BodyNode | null;
  after: BodyNode | null;
  options: BodyDiffOptions;
}

self.addEventListener('message', (event: MessageEvent<Asked>) => {
  const { id, before, after, options } = event.data;
  self.postMessage({ id, diff: diffBodies(before, after, options) });
});
