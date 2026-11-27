/**
 * Whether code blocks show the numbers of their lines: the reader's choice,
 * off unless they turn it on (the switch in the settings, which holds it as
 * state: code-lines.svelte.ts).
 *
 * It is the reader's, not the document's: nothing of it is stored in a
 * body. It is remembered on the device, as the layout readout is, and not
 * with the account: the account's settings are a list the server keeps, and
 * this is not on it. A reader by a public link, who has chosen nothing on
 * this device, sees no numbers.
 *
 * The choice is put on the root element (`data-code-lines`), where the style
 * sheet shows the numbers of a body that is only shown, and where an editor
 * watches it to draw them (`onCodeLinesChange`). Nothing here is of the
 * framework: what draws a body that is only shown reads it too.
 */

/** The attribute of the root element that is there while the numbers are shown. */
export const CODE_LINES_ATTRIBUTE = 'data-code-lines';

/** Where the device remembers the choice. */
export const CODE_LINES_STORAGE_KEY = 'fvoci:code-lines';

/** The choice remembered on this device; off when there is none, or nothing can be read. */
export function rememberedCodeLines(): boolean {
  try {
    return localStorage.getItem(CODE_LINES_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

/** Shows or hides the numbers on the page, and remembers the choice on the device. */
export function showCodeLines(shown: boolean, page: Document = document): void {
  page.documentElement.toggleAttribute(CODE_LINES_ATTRIBUTE, shown);
  try {
    localStorage.setItem(CODE_LINES_STORAGE_KEY, shown ? '1' : '0');
  } catch {
    // Not remembered: it still holds for this visit.
  }
}

/** How many lines a text has: one more than its line feeds. */
export function lineCount(text: string): number {
  let lines = 1;
  for (let at = text.indexOf('\n'); at !== -1; at = text.indexOf('\n', at + 1)) lines += 1;
  return lines;
}

/**
 * How many digits the number of the last line has: how wide the room for
 * the numbers is, given to a block as `data-line-digits` (editor.css)
 * from its first drawing, so that it does not grow when they come.
 */
export const lineDigits = (lines: number): number => String(Math.max(lines, 1)).length;

/** Whether the numbers are shown on this page now. */
export const codeLinesShown = (page: Document): boolean =>
  page.documentElement.hasAttribute(CODE_LINES_ATTRIBUTE);

/** Calls back with the new answer whenever the numbers are turned on or off. Returns what stops it. */
export function onCodeLinesChange(page: Document, callback: (shown: boolean) => void): () => void {
  if (typeof MutationObserver === 'undefined') return () => undefined;
  let last = codeLinesShown(page);
  const observer = new MutationObserver(() => {
    const now = codeLinesShown(page);
    if (now === last) return;
    last = now;
    callback(now);
  });
  observer.observe(page.documentElement, {
    attributes: true,
    attributeFilter: [CODE_LINES_ATTRIBUTE],
  });
  return () => {
    observer.disconnect();
  };
}
