/**
 * What the tests of code blocks share: the coloured runs of a drawn block,
 * read the same way from the editor and from a body that is only shown, and
 * a clipboard that keeps what is written to it.
 */

/** The runs of a drawn block's text as `[class, text]`, `''` for plain; neighbours of one class are one run. */
export function codeRuns(element: Element | null | undefined): [string, string][] {
  const runs: [string, string][] = [];
  if (!element) return runs;
  const walker = element.ownerDocument.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const value = node.nodeValue ?? '';
    if (value === '') continue;
    const coloured = node.parentElement?.closest('[class*="doc-code-"]');
    const kind =
      coloured && element.contains(coloured) && coloured !== element
        ? ([...coloured.classList].find((name) => name.startsWith('doc-code-')) ?? '')
        : '';
    const last = runs.at(-1);
    if (last?.[0] === kind) last[1] += value;
    else runs.push([kind, value]);
  }
  return runs;
}

/**
 * Where in the text the numbers of the lines are drawn: the offset of the
 * text before each, in order. Lines start at 0 and after each line feed.
 */
export function lineNumbers(element: Element | null | undefined): number[] {
  const offsets: number[] = [];
  if (!element) return offsets;
  let seen = 0;
  const walker = element.ownerDocument.createTreeWalker(
    element,
    NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT,
  );
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (node.nodeType === Node.TEXT_NODE) seen += node.nodeValue?.length ?? 0;
    else if ((node as Element).classList.contains('doc-code-line')) offsets.push(seen);
  }
  return offsets;
}

/** Gives the page a clipboard that keeps what is written to it, and returns that list. */
export function clipboardOf(page: Window): string[] {
  const written: string[] = [];
  Object.defineProperty(page.navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText: (value: string) => {
        written.push(value);
        return Promise.resolve();
      },
    },
  });
  return written;
}

/** Takes the clipboard away again. */
export function noClipboard(page: Window): void {
  Object.defineProperty(page.navigator, 'clipboard', { configurable: true, value: undefined });
}
