/**
 * How a code block's text is drawn with its colours and the numbers of its
 * lines: the read-only view builds these elements itself (code-view.ts), and
 * the editor has its own build the same ones from decorations
 * (code-highlight.ts), so that a block looks alike in both.
 *
 * A run of one colour is a `<span>` of the kind's class around its text. The
 * number of a line is an empty element at the start of the line, before the
 * runs of that line, which the style sheet writes the number into and sets in
 * the gutter at the height of the line's start: a line that wraps keeps its
 * number on its first row, the number is never part of the text that is
 * selected or copied, and it is hidden from assistive technology. The numbers
 * are drawn only while the reader has asked for them (code-lines.ts).
 *
 * Above the text, in both, is a bar with the language and the copy button.
 */
import { tokenClassAt, type CodeTokens } from './code-tokens.ts';

/** The class of the element that carries a line's number. */
export const LINE_CLASS = 'doc-code-line';

/** Where each line of a text starts: at 0, and after every line feed (the last line may be empty). */
export function lineStarts(text: string): number[] {
  const starts = [0];
  for (let at = text.indexOf('\n'); at !== -1; at = text.indexOf('\n', at + 1)) starts.push(at + 1);
  return starts;
}

/**
 * The element that shows the number of a line. Every one is the same: the
 * style sheet counts them, so that a line added or taken away changes one
 * element and not the numbers of all the lines after it.
 */
export function lineMarker(page: Document): HTMLElement {
  const marker = page.createElement('span');
  marker.setAttribute('class', LINE_CLASS);
  marker.setAttribute('aria-hidden', 'true');
  marker.setAttribute('contenteditable', 'false');
  return marker;
}

/**
 * How far the editor numbers a block's lines one by one: its lines times its
 * letters. The editor draws a block's text in pieces cut at every number and
 * every colour, and each cut costs about as much as the text after it
 * (ProseMirror's `TextNode.cut`), so that numbering every line of a long
 * block would cost the square of its length at every key. Past this, the
 * editor's view of the block numbers it in one column beside lines that do
 * not wrap (`lineColumn`), which is no part of the editor's drawing and
 * costs nothing as the text changes. A body that is only shown has no such
 * cost and numbers every line.
 */
export const LINE_MARKERS_UP_TO = 8_000_000;

/** Whether the editor numbers a block of so many lines and letters in one column. */
export const numberedInColumn = (lines: number, length: number): boolean =>
  lines * length > LINE_MARKERS_UP_TO;

/**
 * The column of the numbers of a long block's lines, in the editor, beside
 * the block's text and out of it: the style sheet writes them from
 * `data-numbers`, so that it holds no text to select, copy or read out.
 */
export function lineColumn(page: Document): HTMLElement {
  const column = page.createElement('span');
  column.setAttribute('class', 'doc-code-column');
  column.setAttribute('aria-hidden', 'true');
  return column;
}

/** Numbers the column for so many lines, or empties it with 0. */
export function numberColumn(column: HTMLElement, lines: number): void {
  if (lines === 0) column.removeAttribute('data-numbers');
  else
    column.setAttribute(
      'data-numbers',
      Array.from({ length: lines }, (_, index) => index + 1).join('\n'),
    );
}

/**
 * A stretch of a block's text that a comparison marks (render-diff.ts): from
 * `from` to `to`, inside a copy of `element` (an `<ins>` or a `<del>`).
 */
export interface CodeChange {
  from: number;
  to: number;
  element: Element;
}

/**
 * Fills a `<code>` with its text, a run of a colour in an element of its
 * kind, and, with `numbered`, the numbers of the lines. Like `renderBody`, it
 * holds an empty last line open with a `<br>`. What a comparison marked
 * (`changes`, in order) stays marked, around its runs and numbers.
 */
export function paintCode(
  code: HTMLElement,
  text: string,
  tokens: CodeTokens,
  numbered: boolean,
  changes: readonly CodeChange[] = [],
): void {
  const page = code.ownerDocument;
  const parts: Node[] = [];
  const starts = numbered ? lineStarts(text) : [];
  let line = 0;
  let changeAt = 0;
  let open: { element: Element; to: number } | undefined;
  /** Puts down a node that starts at `at`: inside the change it is in, if any. */
  const place = (node: Node, at: number): void => {
    while ((changes[changeAt]?.to ?? Infinity) <= at) changeAt += 1;
    const change = changes[changeAt];
    if (!change || change.from > at) {
      open = undefined;
      parts.push(node);
      return;
    }
    if (open?.to !== change.to) {
      open = { element: change.element.cloneNode(false) as Element, to: change.to };
      parts.push(open.element);
    }
    open.element.append(node);
  };
  /** Where the next change starts or ends after `at`. */
  const changeEdge = (at: number): number => {
    const change = changes.find((one) => one.to > at);
    if (!change) return Infinity;
    return change.from > at ? change.from : change.to;
  };
  const number = (at: number): void => {
    line += 1;
    place(lineMarker(page), at);
  };
  /** Puts down the text from `from` to `to`, in a run of `kind` if there is one, numbering its lines. */
  const put = (from: number, to: number, kind: number | undefined): void => {
    const className = kind === undefined ? undefined : tokenClassAt(kind);
    for (let at = from; at < to;) {
      if (starts[line] === at) number(at);
      // A run that goes over the end of a line is cut there, for the number of the next one.
      const next = Math.min(starts[line] ?? to, to, changeEdge(at));
      const piece = page.createTextNode(text.slice(at, next));
      if (className === undefined) place(piece, at);
      else {
        const run = page.createElement('span');
        run.setAttribute('class', className);
        run.append(piece);
        place(run, at);
      }
      at = next;
    }
  };
  let at = 0;
  for (let index = 0; index + 2 < tokens.length; index += 3) {
    const from = tokens[index] ?? 0;
    const to = tokens[index + 1] ?? 0;
    put(at, from, undefined);
    put(from, to, tokens[index + 2]);
    at = to;
  }
  put(at, text.length, undefined);
  // The last line has no text when the block is empty or ends in a line feed.
  if (line < starts.length) number(text.length);
  if (text === '' || text.endsWith('\n')) parts.push(page.createElement('br'));
  code.replaceChildren(...parts);
}

/** The frame of a code block: the bar with the language and the copy button, then the block. */
export function codeFrame(page: Document, bar: HTMLElement, pre: HTMLElement): HTMLElement {
  const frame = page.createElement('div');
  frame.setAttribute('class', 'doc-code');
  frame.append(bar, pre);
  return frame;
}

/** A bar that holds `parts`: kept out of the text's way, and of the editor's. */
export function codeBar(page: Document, ...parts: Node[]): HTMLElement {
  const bar = page.createElement('div');
  bar.setAttribute('class', 'doc-code-bar');
  bar.setAttribute('contenteditable', 'false');
  bar.append(...parts);
  return bar;
}
