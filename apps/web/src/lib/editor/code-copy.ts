/**
 * The copy button of a code block, in the editor and where a body is only
 * shown: it puts the block's whole text on the clipboard as it is stored,
 * without the numbers of the lines (they are no text).
 *
 * The button says that it copied with a tick for a moment, and a line beside
 * it that is read out (`role="status"`). Where the page may not write to the
 * clipboard (no secure origin, a browser that refuses), the browser's own
 * copy of a selection is tried; when that fails too, the text is left
 * selected and the line says to copy it with the keys. It never fails
 * without saying so.
 *
 * Its words are given (code-copy-texts.ts): what draws a body that is only
 * shown, fetched when the body has code, holds no messages of its own.
 */

/** How long the tick and the line stay, in milliseconds; what to do instead takes longer to read. */
const COPIED_FOR = 2000;
const FAILED_FOR = 8000;

const SVG = 'http://www.w3.org/2000/svg';

/** An icon of the button, from the shapes it is drawn with (Lucide's `copy` and `check`). */
function icon(
  page: Document,
  name: string,
  shapes: [string, Record<string, string>][],
): SVGElement {
  const svg = page.createElementNS(SVG, 'svg');
  for (const [attribute, value] of Object.entries({
    class: `doc-code-icon doc-code-icon-${name}`,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    'stroke-width': '2',
    'stroke-linecap': 'round',
    'stroke-linejoin': 'round',
    'aria-hidden': 'true',
  }))
    svg.setAttribute(attribute, value);
  for (const [tag, attributes] of shapes) {
    const shape = page.createElementNS(SVG, tag);
    for (const [attribute, value] of Object.entries(attributes))
      shape.setAttribute(attribute, value);
    svg.append(shape);
  }
  return svg;
}

export interface CopyControls {
  button: HTMLButtonElement;
  /** The line that says what became of a press; read out. */
  status: HTMLElement;
  destroy(): void;
}

/** What the button says, read at each use, so that it is in the reader's language of the moment. */
export interface CopyTexts {
  /** The button's name. */
  copy: () => string;
  /** The line once the text is on the clipboard. */
  copied: () => string;
  /** The line when it could not be put there: the text is selected, to be copied with the keys. */
  failed: () => string;
}

export interface CopyOptions {
  texts: CopyTexts;
  /** The text to copy, as it is at the press. */
  text: () => string;
  /** Selects the block's text, for the browser's copy and for the reader's keys. */
  select: () => void;
}

/** Writes the text with the clipboard's interface, or with the browser's copy of the selection. */
async function write(page: Document, text: string, select: () => void): Promise<boolean> {
  const clipboard = page.defaultView?.navigator.clipboard;
  if (clipboard) {
    try {
      await clipboard.writeText(text);
      return true;
    } catch {
      // Refused (no secure origin, no permission): the selection is the other way.
    }
  }
  select();
  try {
    // eslint-disable-next-line @typescript-eslint/no-deprecated -- the one way left where the clipboard's interface is not given
    return page.execCommand('copy');
  } catch {
    return false;
  }
}

/** The button and its line. */
export function copyControls(page: Document, options: CopyOptions): CopyControls {
  const button = page.createElement('button');
  button.setAttribute('type', 'button');
  button.setAttribute('class', 'doc-code-copy');
  button.setAttribute('aria-label', options.texts.copy());
  button.setAttribute('title', options.texts.copy());
  button.append(
    icon(page, 'copy', [
      ['rect', { width: '14', height: '14', x: '8', y: '8', rx: '2', ry: '2' }],
      ['path', { d: 'M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2' }],
    ]),
    icon(page, 'check', [['path', { d: 'M20 6 9 17l-5-5' }]]),
  );
  const status = page.createElement('span');
  status.setAttribute('class', 'doc-code-status');
  status.setAttribute('role', 'status');

  let timer: ReturnType<typeof setTimeout> | undefined;
  let gone = false;
  const say = (words: string, copied: boolean): void => {
    clearTimeout(timer);
    button.toggleAttribute('data-copied', copied);
    status.textContent = words;
    timer = setTimeout(
      () => {
        button.removeAttribute('data-copied');
        status.textContent = '';
      },
      copied ? COPIED_FOR : FAILED_FOR,
    );
  };
  button.addEventListener('click', (event) => {
    event.preventDefault();
    void write(page, options.text(), options.select).then((copied) => {
      if (gone) return;
      if (copied) say(options.texts.copied(), true);
      else {
        options.select();
        say(options.texts.failed(), false);
      }
    });
  });
  return {
    button,
    status,
    destroy: () => {
      gone = true;
      clearTimeout(timer);
    },
  };
}
