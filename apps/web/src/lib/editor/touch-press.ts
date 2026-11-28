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

/** How long a finger rests on an element before the press is a long one, in milliseconds. */
export const LONG_PRESS_MS = 500;
/** How far the finger may move meanwhile, in pixels: further, and it scrolls. */
const LONG_PRESS_SLOP = 10;
/**
 * What a long press inside the element keeps doing what it does: a link (the
 * browser's own menu of it), a button, a field, a text being written.
 */
const PRESSED_ON_ITS_OWN = 'a, button, input, select, textarea, [contenteditable="true"]';

/**
 * An attachment: a finger that rests on the element (a comment) for
 * `LONG_PRESS_MS` calls `onLongPress`, in place of the browser's own menu of
 * the press. The click that ends the press is kept from what was pressed. A
 * finger that moves scrolls as before; a mouse and a pen press as before; and
 * so does a finger on a link or a control inside (`PRESSED_ON_ITS_OWN`).
 */
export function longPress(onLongPress: () => void): (element: HTMLElement) => () => void {
  return (element) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let origin: { x: number; y: number } | undefined;
    // A long press answered: the browser's menu of it is kept from the reader until
    // another press, and the click that ends it from what was pressed.
    let menuKept = false;
    let clickKept = false;
    const stop = (): void => {
      clearTimeout(timer);
      timer = undefined;
      origin = undefined;
    };
    const down = (event: PointerEvent): void => {
      if (!event.isPrimary) return;
      // Another press: a long one before it that ended in no click (the finger was taken
      // over) keeps no click from this one, on a link or with a mouse either.
      menuKept = false;
      clickKept = false;
      if (event.pointerType !== 'touch') return;
      if (event.target instanceof Element && event.target.closest(PRESSED_ON_ITS_OWN)) return;
      stop();
      origin = { x: event.clientX, y: event.clientY };
      timer = setTimeout(() => {
        stop();
        menuKept = true;
        clickKept = true;
        onLongPress();
      }, LONG_PRESS_MS);
    };
    const move = (event: PointerEvent): void => {
      if (!origin) return;
      if (Math.hypot(event.clientX - origin.x, event.clientY - origin.y) > LONG_PRESS_SLOP) stop();
    };
    // The browser took the finger over: no click ends the press, so none is kept from
    // what comes next (Enter or Space on a control). Its menu may still come.
    const cancel = (): void => {
      stop();
      clickKept = false;
    };
    const menu = (event: Event): void => {
      if (timer !== undefined || menuKept) event.preventDefault();
    };
    const click = (event: Event): void => {
      if (!clickKept) return;
      clickKept = false;
      event.preventDefault();
      event.stopPropagation();
    };
    element.addEventListener('pointerdown', down);
    element.addEventListener('pointermove', move);
    element.addEventListener('pointerup', stop);
    element.addEventListener('pointercancel', cancel);
    element.addEventListener('contextmenu', menu);
    element.addEventListener('click', click, true);
    return () => {
      stop();
      element.removeEventListener('pointerdown', down);
      element.removeEventListener('pointermove', move);
      element.removeEventListener('pointerup', stop);
      element.removeEventListener('pointercancel', cancel);
      element.removeEventListener('contextmenu', menu);
      element.removeEventListener('click', click, true);
    };
  };
}
