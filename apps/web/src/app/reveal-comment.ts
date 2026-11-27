/**
 * Shows a comment that is on its way: the card or page it is on opens, its
 * comments are read, and then it is there (`data-comment` of the list). Once
 * it is, it is scrolled into the middle and marked for a moment
 * (`data-revealed`). While marked, if the body above it grows as its editor
 * mounts, it is kept in the middle unless the reader starts scrolling or
 * interacting. A comment that does not come within `wait` (gone since, or
 * the card did not open) is let go of.
 */

/** How long the mark stays on the comment. */
const MARKED = 2000;

/** The scrolling content child whose height grows as the body above `comment` finishes mounting. */
function flowOf(comment: Element): Element | undefined {
  let node: Element | null = comment.parentElement;
  let child: Element = comment;
  while (node && node !== document.body) {
    const overflow = getComputedStyle(node).overflowY;
    if (overflow === 'auto' || overflow === 'scroll') return child;
    child = node;
    node = node.parentElement;
  }
  return comment.closest('article') ?? undefined;
}

/** The function that stops waiting (or keeping the revealed comment in sight). */
export function revealComment(commentId: string, wait = 10_000): () => void {
  const selector = `[data-comment="${CSS.escape(commentId)}"]`;
  let settled: (() => void) | undefined;

  const show = (comment: Element): void => {
    const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
    // Flush layout before scrolling in WebKit while lazy stylesheets are still arriving.
    comment.getBoundingClientRect();
    comment.scrollIntoView({ block: 'center', behavior: still ? 'auto' : 'smooth' });
    comment.setAttribute('data-revealed', '');

    let resizer: ResizeObserver | undefined;
    const stopFollowing = (): void => {
      resizer?.disconnect();
      resizer = undefined;
      for (const type of ['wheel', 'touchmove', 'pointerdown', 'keydown'] as const) {
        window.removeEventListener(type, stopFollowing, true);
      }
    };
    const flow = flowOf(comment);
    if (flow && typeof ResizeObserver === 'function') {
      let lastHeight = flow.getBoundingClientRect().height;
      resizer = new ResizeObserver(() => {
        if (!comment.isConnected) {
          stopFollowing();
          return;
        }
        const height = flow.getBoundingClientRect().height;
        if (Math.abs(height - lastHeight) < 1) return;
        lastHeight = height;
        comment.scrollIntoView({ block: 'center', behavior: 'auto' });
      });
      resizer.observe(flow);
      for (const type of ['wheel', 'touchmove', 'pointerdown', 'keydown'] as const) {
        window.addEventListener(type, stopFollowing, { passive: true, capture: true });
      }
    }

    const clearTimer = setTimeout(() => {
      stopFollowing();
      comment.removeAttribute('data-revealed');
    }, MARKED);
    settled = () => {
      stopFollowing();
      clearTimeout(clearTimer);
    };
  };

  const there = document.querySelector(selector);
  if (there) {
    show(there);
    return () => {
      settled?.();
    };
  }
  const observer = new MutationObserver(() => {
    const comment = document.querySelector(selector);
    if (!comment) return;
    stopWaiting();
    show(comment);
  });
  const timer = setTimeout(stopWaiting, wait);
  function stopWaiting(): void {
    observer.disconnect();
    clearTimeout(timer);
  }
  observer.observe(document.body, { childList: true, subtree: true });
  return () => {
    stopWaiting();
    settled?.();
  };
}
