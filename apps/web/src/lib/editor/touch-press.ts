/**
 * A finger's press that opens something from a body (a file's name, a
 * reference) in a body that is not being written: it opens what it opens and
 * does not start the writing, for on a phone the focus of the body is its
 * keyboard, which would come up (if only for a moment, before the page
 * moves on) and cover half the screen.
 *
 * The press is kept from the browser, which would focus the editing host
 * (WebKit, through the element pressed), and from the editor, which would
 * select what was pressed and take the focus: a pressed event whose default
 * is prevented is left alone by both. Its click still comes and does what the
 * element does. A finger in a body being written, and a mouse anywhere, press
 * as before: what is pressed is selected (and can be dragged by a mouse).
 */
export function keepTouchFromText(element: HTMLElement): void {
  let pointer = '';
  element.addEventListener('pointerdown', (event) => {
    pointer = event.pointerType;
  });
  element.addEventListener('mousedown', (event) => {
    const text = element.closest('[contenteditable="true"]');
    if (pointer === 'touch' && text !== null && element.ownerDocument.activeElement !== text) {
      event.preventDefault();
    }
  });
}
