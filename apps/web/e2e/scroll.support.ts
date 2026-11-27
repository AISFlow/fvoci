/** Wait for the browser to deliver the scroll, so a keeper has read it before a resize. */
import type { Locator } from '@playwright/test';

export async function scrollTo(target: Locator, left: number, top: number): Promise<void> {
  await target.evaluate(
    (element, place) =>
      new Promise<void>((resolve) => {
        if (element.scrollLeft === place.left && element.scrollTop === place.top) {
          resolve();
          return;
        }
        element.addEventListener(
          'scroll',
          () => {
            resolve();
          },
          { once: true },
        );
        element.scrollTo(place.left, place.top);
      }),
    { left, top },
  );
}
