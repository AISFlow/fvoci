/** What an observer of an element's size is told, against what is true of the element by then. */

/**
 * Whether an element still has the box its observer is being told of. It has not when an
 * observer that ran before changed the page in answer (the stage of the page is worked out
 * that way): what the observer then reads is laid out anew, at another size.
 */
export function stillLaidOut(element: HTMLElement, told: ResizeObserverEntry | undefined): boolean {
  const box = told?.borderBoxSize[0];
  if (!box) return true;
  return (
    Math.abs(box.inlineSize - element.offsetWidth) < 1 &&
    Math.abs(box.blockSize - element.offsetHeight) < 1
  );
}
