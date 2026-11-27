/**
 * What the tests of the carets ask of a body on the page, in memory and on the
 * real server alike.
 */
import type { Locator } from '@playwright/test';

/**
 * Scrolls what `line` scrolls in until its top is `below` pixels under that box's top; says
 * whether it could.
 */
export const scrollTo = (line: Locator, below = 0): Promise<boolean> =>
  line.evaluate((element, by) => {
    let box = element.parentElement;
    while (
      box &&
      !(/auto|scroll/.test(getComputedStyle(box).overflowY) && box.scrollHeight > box.clientHeight)
    )
      box = box.parentElement;
    const scroller = box ?? document.scrollingElement ?? document.documentElement;
    const top = box ? box.getBoundingClientRect().top : 0;
    scroller.scrollTop += element.getBoundingClientRect().top - top - by;
    return Math.abs(element.getBoundingClientRect().top - top - by) < 1;
  }, below);

/** What is seen at the top of the name is the body: nothing is drawn over it, nor does it go past an edge. */
export const uncovered = (editor: Locator): Promise<boolean> =>
  editor.evaluate((body) => {
    const name = body.querySelector('.doc-caret-name')?.getBoundingClientRect();
    const seen = name && document.elementFromPoint(name.left + 1, name.top + 1);
    return Boolean(seen && body.contains(seen));
  });
