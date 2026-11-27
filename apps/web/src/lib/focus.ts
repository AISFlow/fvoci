/** Where the focus is, when the answer is "nowhere", and moving it onto what was just revealed. */

/**
 * Whether the focus is on nothing: on the page itself, or on a box around `home` that only
 * holds it. Safari does not focus a link or a button that a mouse presses, and leaves the
 * focus on the nearest box around it that can hold one; when what was pressed then goes away,
 * the focus is no more anywhere than it is on the page in the other engines. `home` is where
 * the focus would be sent.
 */
export function focusAdrift(home: Element | null | undefined): boolean {
  const active = document.activeElement;
  if (!active || active === document.body) return true;
  if (!home || active === home || !(active instanceof HTMLElement)) return false;
  return active.tabIndex < 0 && active.contains(home);
}

/**
 * Moves the focus to an element that was just unhidden (`visibility: hidden`) or freed from
 * `inert`. WebKit skips style recalculation inside `focus()` while any `<link rel="stylesheet">`
 * (such as a lazy font sheet) is pending, so `isFocusable()` still sees the stale hidden or
 * inert style and drops focus to `document.body`. Calling `getBoundingClientRect()` forces
 * style and layout to catch up regardless of pending sheets.
 */
export function focusRevealed(
  element: HTMLElement | null | undefined,
  options?: FocusOptions,
): void {
  if (!element?.isConnected) return;
  element.getBoundingClientRect();
  element.focus(options);
}

/**
 * What opened a sheet, a dialog or a drawer, for the focus to go back to once it closes. Safari
 * does not focus a link or a button that a mouse or a finger presses, so what has the focus
 * when it opens may be the page itself: what was pressed is taken from the event instead.
 */
export class Opener {
  #element: HTMLElement | null = null;

  /** Remembers what `event` was on, else what has the focus now (the page itself is nothing). */
  remember(event?: Event): void {
    const target = event?.currentTarget;
    this.#element = target instanceof HTMLElement ? target : focusedElement();
  }

  /** Remembers `element`, in place of what an event would say. */
  hold(element: HTMLElement | null | undefined): void {
    this.#element = element ?? null;
  }

  /** What opened, while it is still on the page; forgotten once taken. */
  take(): HTMLElement | null {
    const element = this.#element;
    this.#element = null;
    return element?.isConnected ? element : null;
  }
}

/** The element that has the focus; none where that is the page itself. */
function focusedElement(): HTMLElement | null {
  const active = document.activeElement;
  return active instanceof HTMLElement && active !== document.body ? active : null;
}
