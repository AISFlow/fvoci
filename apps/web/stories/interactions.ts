import { expect, userEvent, waitFor } from 'storybook/test';

/** Portals belong to the story's document, not to its canvas element. */
export async function storyElement(canvas: HTMLElement, selector: string): Promise<HTMLElement> {
  return waitFor(() => {
    const element = canvas.ownerDocument.querySelector<HTMLElement>(selector);
    if (!element) throw new Error(`Story element not ready: ${selector}`);
    return element;
  });
}

/**
 * Resolves once `count` embeds under the canvas are no longer being drawn: each
 * drawn, or showing why it is not. The second that `waitFor` allows is no
 * measure of it: the library is fetched first, and diagrams are drawn one after
 * another. It has no limit of its own: a drawing that never ends leaves the
 * story unfinished, which whoever runs the story is told (the browser check
 * waits for the story's end).
 */
export function embedsSettled(canvas: HTMLElement, count: number): Promise<void> {
  const settled = (): boolean =>
    canvas.querySelectorAll('[data-state]:not([data-state="pending"])').length >= count;
  return new Promise((resolve) => {
    if (settled()) {
      resolve();
      return;
    }
    const observer = new MutationObserver(() => {
      if (!settled()) return;
      observer.disconnect();
      resolve();
    });
    observer.observe(canvas, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['data-state'],
    });
  });
}

export async function clickStory(canvas: HTMLElement, selector: string): Promise<void> {
  await userEvent.click(await visibleStoryElement(canvas, selector));
}

/** Surface elements exist before their opening transition makes them visible. */
export async function visibleStoryElement(
  canvas: HTMLElement,
  selector: string,
): Promise<HTMLElement> {
  const element = await storyElement(canvas, selector);
  await waitFor(() => expect(element).toBeVisible());
  return element;
}

/**
 * Selects two cards of a view the way a finger can: "Select" in the menu of
 * the first (`menu` opens it, once `first` has the focus: the button waits
 * for it under a mouse), then a plain press on the second. Resolves with the
 * bar of the selection, once it counts both, and checks that the bar fits
 * its view across.
 */
export async function selectTwo(
  canvas: HTMLElement,
  { first, menu, second }: { first: string; menu: string; second: string },
): Promise<HTMLElement> {
  (await storyElement(canvas, first)).focus();
  await clickStory(canvas, menu);
  await clickStory(canvas, '[data-action="select"]');
  // A menu that closes holds the page's pointer until it has gone.
  const body = canvas.ownerDocument.body;
  await waitFor(() => expect(getComputedStyle(body).pointerEvents).not.toBe('none'));
  await clickStory(canvas, second);
  const bar = await storyElement(canvas, '[data-testid="selection-bar"]');
  await waitFor(() => expect(bar).toHaveTextContent('2'));
  await expect(bar.scrollWidth).toBeLessThanOrEqual(bar.clientWidth);
  const view = canvas.querySelector<HTMLElement>('[data-testid="view-example"]');
  if (view) {
    const [box, room] = [bar.getBoundingClientRect(), view.getBoundingClientRect()];
    await expect(box.left).toBeGreaterThanOrEqual(room.left);
    await expect(box.right).toBeLessThanOrEqual(room.right);
    // The button that lets the selection go is always in view.
    const clear = await storyElement(canvas, '[data-bulk-clear]');
    await expect(clear.getBoundingClientRect().right).toBeLessThanOrEqual(room.right);
  }
  return bar;
}
