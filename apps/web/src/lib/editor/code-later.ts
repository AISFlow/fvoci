/**
 * Fetches what draws the code blocks of a body that is only shown
 * (code-view.ts) when the body has one, and only then: a body without code
 * costs a look for a selector. The room for each block's bar is in the style
 * sheet from the start (editor.css), so that its coming moves nothing.
 */
import type { CopyTexts } from './code-copy.ts';

/** The code blocks of a body as `renderBody` draws them, diagrams aside. */
export const CODE_BLOCK_SELECTOR = 'pre > code:not(.language-mermaid)';

/**
 * Draws the code blocks under `root` once fetched, with `texts` for their
 * copy buttons. Returns what undoes it. `_placed` is the nodes of the body:
 * an attachment that is given them runs again when they change.
 */
export function drawCodeBlocksLater(
  root: HTMLElement,
  texts: CopyTexts,
  ..._placed: unknown[]
): () => void {
  if (!root.querySelector(CODE_BLOCK_SELECTOR)) return () => undefined;
  let undo: (() => void) | undefined;
  let gone = false;
  import('./code-view.ts').then(
    ({ drawCodeBlocks }) => {
      if (!gone) undo = drawCodeBlocks(root, texts);
    },
    // Not fetched: the blocks stay as `renderBody` drew them.
    () => undefined,
  );
  return () => {
    gone = true;
    undo?.();
  };
}
