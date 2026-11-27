/**
 * A text box as high as its text.
 *
 * The style sheet asks for that (`field-sizing-content`), and a browser that
 * knows the property needs nothing else. For one that does not, this sets
 * the height from the text whenever the text or the width of the box
 * changes.
 *
 * Use: `{@attach autosize(() => text)}` on a `<textarea>`. The function is
 * read so that the box is fitted again when the text is replaced from the
 * outside (a comment was sent, somebody else renamed the card), which sends
 * no input event.
 */

function sizesItself(): boolean {
  return typeof CSS !== 'undefined' && typeof CSS.supports === 'function'
    ? CSS.supports('field-sizing', 'content')
    : false;
}

/** Sets the height of a text box to the height of its text. */
export function fitHeight(element: HTMLTextAreaElement): void {
  element.style.height = 'auto';
  element.style.height = `${element.scrollHeight}px`;
}

export function autosize(
  text: () => string,
): (element: HTMLTextAreaElement) => (() => void) | undefined {
  return (element) => {
    // Read here: the attachment runs again when the text changes.
    text();
    if (sizesItself()) return undefined;
    fitHeight(element);
    const fit = (): void => {
      fitHeight(element);
    };
    element.addEventListener('input', fit);
    if (typeof ResizeObserver === 'undefined') {
      return () => {
        element.removeEventListener('input', fit);
      };
    }
    // The same text wraps differently in another width. The height is this
    // function's own doing and is not followed, or it would never settle.
    let width = element.clientWidth;
    const observer = new ResizeObserver(() => {
      if (element.clientWidth === width) return;
      width = element.clientWidth;
      fitHeight(element);
    });
    observer.observe(element);
    return () => {
      element.removeEventListener('input', fit);
      observer.disconnect();
    };
  };
}
