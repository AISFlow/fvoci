/**
 * How a file block is drawn, in the editor and where a body is only shown:
 * an icon for its kind, its name and size, and a link that saves it under
 * its name. The document holds the file's identifier alone, so the rest is
 * put into the block's element here, from what the file source
 * (`files.ts`) knows of the file. The name of a file the viewer shows is a
 * button that opens it there, where the app has a viewer (`fileViewer`);
 * pressed down, it is still the block's, which it selects and drags.
 *
 * A block is in one of three states (`data-state`): `pending` while its
 * file's record is being asked, `shown` with the name and size, and
 * `missing` for a file that is not there (deleted, or not of the board or
 * wiki shown), which has nothing to save. A record that cannot be asked
 * now leaves the block pending, still to be saved by its identifier.
 *
 * Everything is made as elements: no text is read as markup.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { humanFileSize } from '$lib/logic/file-size.ts';
import { saveBlob } from '$lib/save.ts';

import { FILE_BLOCK_ATTRIBUTE, FILE_ID_ATTRIBUTE } from './drawing.ts';
import {
  fileBytes,
  fileDetails,
  fileUrl,
  fileViewer,
  forgetFile,
  type FileDetails,
} from './files.ts';
import { keepTouchFromText } from './touch-press.ts';

/** The places of file blocks, in the editor and in a body that is shown. */
export const FILE_BLOCK_SELECTOR = `div[${FILE_BLOCK_ATTRIBUTE}]`;
/** Marks a place that has been drawn into. */
const DRAWN = 'data-drawn';
const SVG = 'http://www.w3.org/2000/svg';

type Kind = 'document' | 'sheet' | 'slides' | 'archive' | 'image' | 'audio' | 'other';

/** The extensions of the files of each kind but `other`. */
const EXTENSIONS: Record<Exclude<Kind, 'other'>, string> = {
  document: 'pdf hwp hwpx doc docx odt rtf txt md',
  sheet: 'xls xlsx ods csv',
  slides: 'ppt pptx odp key',
  archive: 'zip 7z rar tar gz',
  image: 'png jpg jpeg gif webp avif svg heic',
  audio: 'mp3 wav m4a ogg flac',
};

const KINDS = new Map(
  Object.entries(EXTENSIONS).flatMap(([kind, names]) =>
    names.split(' ').map((name) => [name, kind as Kind] as const),
  ),
);

const SHEET =
  'M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z';
const FOLD = 'M14 2v5a1 1 0 0 0 1 1h5';

/**
 * The outlines of the icons (Lucide's, ISC), each a list of SVG elements:
 * a path by its outline, or a circle of radius 2 by its centre (`x y`).
 */
const ICONS: Record<Kind | 'download', string[]> = {
  other: [SHEET, FOLD],
  document: [SHEET, FOLD, 'M10 9H8', 'M16 13H8', 'M16 17H8'],
  sheet: [SHEET, FOLD, 'M8 13h2', 'M14 13h2', 'M8 17h2', 'M14 17h2'],
  slides: ['M2 3h20', 'M21 3v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V3', 'm7 21 5-5 5 5'],
  archive: [
    'M13.659 22H18a2 2 0 0 0 2-2V8a2.4 2.4 0 0 0-.706-1.706l-3.588-3.588A2.4 2.4 0 0 0 14 2H6a2 2 0 0 0-2 2v11.5',
    FOLD,
    'M8 12v-1',
    'M8 18v-2',
    'M8 7V6',
    '8 20',
  ],
  image: [SHEET, FOLD, '10 12', 'm20 17-1.296-1.296a2.41 2.41 0 0 0-3.408 0L9 22'],
  audio: [
    'M11.65 22H18a2 2 0 0 0 2-2V8a2.4 2.4 0 0 0-.706-1.706l-3.588-3.588A2.4 2.4 0 0 0 14 2H6a2 2 0 0 0-2 2v10.35',
    FOLD,
    'M8 20v-7l3 1.474',
    '6 20',
  ],
  download: ['M12 15V3', 'M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4', 'm7 10 5 5 5-5'],
};

/** The kind of a file by the extension of its name (or of its identifier, which keeps it). */
export function kindOf(name: string): Kind {
  const dot = name.lastIndexOf('.');
  const extension = dot < 0 ? '' : name.slice(dot + 1).toLowerCase();
  return KINDS.get(extension) ?? 'other';
}

function icon(page: Document, kind: Kind | 'download', name: string): SVGSVGElement {
  const svg = page.createElementNS(SVG, 'svg');
  const attributes = {
    class: name,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    'stroke-width': '2',
    'stroke-linecap': 'round',
    'stroke-linejoin': 'round',
    'aria-hidden': 'true',
    focusable: 'false',
  };
  for (const [key, value] of Object.entries(attributes)) svg.setAttribute(key, value);
  for (const shape of ICONS[kind]) {
    const [x, y] = shape.split(' ');
    const circle = /^[\d.]+ [\d.]+$/.test(shape);
    const part = page.createElementNS(SVG, circle ? 'circle' : 'path');
    if (circle) {
      part.setAttribute('cx', x ?? '');
      part.setAttribute('cy', y ?? '');
      part.setAttribute('r', '2');
    } else part.setAttribute('d', shape);
    svg.append(part);
  }
  return svg;
}

function span(page: Document, name: string, text: string): HTMLElement {
  const element = page.createElement('span');
  element.setAttribute('class', name);
  element.textContent = text;
  return element;
}

/**
 * Saves the file of a press without a modifier under its name, from its
 * bytes; one with a modifier, or a source without bytes, is the browser's
 * own following of the link (the server offers the file as a download).
 * Bytes that cannot be read leave the page where it is (the source tells
 * why): the block is drawn again from what the server says of the file
 * now, so that one gone meanwhile says so.
 */
function onPress(event: MouseEvent, link: HTMLAnchorElement, fileId: string, name: string): void {
  if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) {
    return;
  }
  // A file being read is not read again by another press.
  if (link.getAttribute('aria-busy') === 'true') {
    event.preventDefault();
    return;
  }
  const bytes = fileBytes(fileId);
  if (!bytes) return;
  event.preventDefault();
  link.setAttribute('aria-busy', 'true');
  // Pressed before the file's record came (or after it could not be read), the bytes are
  // saved under its name once that is known, and under the drawn one only failing that.
  void Promise.all([bytes, fileDetails(fileId)])
    .then(
      ([blob, details]) => {
        saveBlob(blob, details?.name ?? name);
      },
      () => {
        const block = link.closest<HTMLElement>(FILE_BLOCK_SELECTOR);
        forgetFile(fileId);
        if (block) drawFileBlock(block);
      },
    )
    .finally(() => {
      link.removeAttribute('aria-busy');
    });
}

/** Set while Enter on the selected block presses its name (`pressFileOf`). */
let pressedFromBlock = false;

/**
 * Keeps what is typed on a block's control (its name, its download) from
 * the text around it. Enter (and Space on the name) press it, and Tab moves
 * on; a key that edits edits nothing from here. WebKit would apply it, and
 * an input method (or text put in) in any browser would type it, where the
 * editor's caret was before the control had the focus: so while it has the
 * focus, the editor's caret is out of the document's selection. The editor
 * keeps its own, and writes it back when the focus comes back to it from
 * the control (file-block.ts). A shortcut of the browser's (find, print) is
 * still the browser's, but not one a browser edits by (bold, italic,
 * underline, paste, cut, undo, redo); AltGr's characters (Ctrl with Alt)
 * are typed, and so are the keys an input method or a dead key takes.
 */
function holdTyping(page: Document, control: HTMLElement): void {
  control.addEventListener('keydown', (event) => {
    const shortcut = (event.ctrlKey || event.metaKey) && !event.altKey;
    const typed = event.key.length === 1 && event.key !== ' ';
    const composed = event.isComposing || ['Process', 'Dead'].includes(event.key);
    if (
      event.key === 'Backspace' ||
      event.key === 'Delete' ||
      composed ||
      (typed && (!shortcut || 'biuvxyz'.includes(event.key.toLowerCase())))
    ) {
      event.preventDefault();
    }
  });
  control.addEventListener('focus', () => {
    const text = control.closest('[contenteditable="true"]');
    const selection = page.getSelection();
    if (text && selection?.anchorNode && text.contains(selection.anchorNode)) {
      selection.removeAllRanges();
    }
  });
}

/**
 * The name of a shown file: a button that opens the viewer on it when the
 * viewer shows its kind, its text otherwise. A viewer that does not come
 * (the connection gone) leaves the file to its download.
 */
function nameOf(
  page: Document,
  fileId: string,
  details: FileDetails | null | undefined,
  name: string,
): HTMLElement {
  const viewer = fileViewer();
  if (!details || !viewer?.opens(details)) return span(page, 'doc-file-name', name);
  const button = page.createElement('button');
  button.setAttribute('type', 'button');
  button.setAttribute('class', 'doc-file-name doc-file-open');
  // Said as the viewer is opened from a card's attachment: "View …".
  const label = m.card_attachment_view({ name });
  button.setAttribute('aria-label', label);
  button.setAttribute('title', label);
  button.textContent = name;
  /** The name drawn for the file now: this one, or the one drawn in its place meanwhile. */
  const drawn = (): HTMLElement | null => {
    if (button.isConnected) return button;
    const blocks = [...page.querySelectorAll<HTMLElement>(FILE_BLOCK_SELECTOR)];
    const again = blocks.find((block) => block.getAttribute(FILE_ID_ATTRIBUTE) === fileId);
    return again?.querySelector<HTMLElement>('button.doc-file-open') ?? null;
  };
  holdTyping(page, button);
  // A finger's press in a body not being written opens the file and leaves the body unfocused.
  keepTouchFromText(button);
  button.addEventListener('click', (event) => {
    const block = button.closest<HTMLElement>(FILE_BLOCK_SELECTOR);
    // Pressed by a pointer that left the focus in the editor (a click, after which the editor has
    // selected the block and taken the focus), or by Enter on the selected block, the editor has
    // the focus back when the viewer closes, whose selection the block still is (Delete deletes
    // it, the arrows move on from it). Pressed from the keyboard on the name, by an assistive
    // technology, or by a pointer that left the focus elsewhere (on the name after a double or a
    // Shift click, or where it was after a finger's press, see touch-press.ts), the name
    // has it back; where a body is only shown, too.
    const text = button.closest('[contenteditable="true"]');
    const toEditor =
      pressedFromBlock || (event.detail > 0 && text !== null && page.activeElement === text);
    const back = (): HTMLElement | null => {
      const now = drawn();
      if (!toEditor || !now) return now;
      return now.closest<HTMLElement>('[contenteditable="true"]') ?? now;
    };
    void viewer.open(fileId, details, back).then((opened) => {
      if (!opened && block) saveFileOf(block);
    });
  });
  return button;
}

/** Draws the block as its file is known now: `undefined` while that is being asked. */
function draw(element: HTMLElement, fileId: string, details: FileDetails | null | undefined): void {
  const page = element.ownerDocument;
  const missing = details === null;
  const name = details ? details.name : missing ? m.file_block_missing() : fileId;
  const size = details ? humanFileSize(details.size) : '';
  element.setAttribute('data-state', details ? 'shown' : missing ? 'missing' : 'pending');
  element.setAttribute('role', 'group');
  element.setAttribute(
    'aria-label',
    size === '' ? m.file_block_label_unsized({ name }) : m.file_block_label({ name, size }),
  );
  const parts: Element[] = [
    icon(page, missing ? 'other' : kindOf(details?.name ?? fileId), 'doc-file-icon'),
    nameOf(page, fileId, details, name),
  ];
  if (size !== '') parts.push(span(page, 'doc-file-size', size));
  const href = missing ? undefined : fileUrl(fileId);
  if (href !== undefined) {
    const link = page.createElement('a');
    link.setAttribute('class', 'doc-file-download');
    link.setAttribute('href', href);
    link.setAttribute('download', name);
    // Pressed, never dragged: dropped into the body, its address would become a link there.
    link.setAttribute('draggable', 'false');
    link.setAttribute('aria-label', m.file_block_download({ name }));
    link.setAttribute('title', m.file_block_download({ name }));
    link.append(icon(page, 'download', 'doc-file-download-icon'));
    link.addEventListener('click', (event) => {
      onPress(event, link, fileId, name);
    });
    holdTyping(page, link);
    parts.push(link);
  }
  element.replaceChildren(...parts);
}

/**
 * Draws a file block into its place, and again once its file's record has
 * been read. The place keeps its identifier; a place without one is left as
 * it is.
 */
export function drawFileBlock(element: HTMLElement): void {
  const fileId = element.getAttribute(FILE_ID_ATTRIBUTE);
  if (fileId === null || fileId === '') return;
  element.setAttribute(DRAWN, '');
  draw(element, fileId, undefined);
  void fileDetails(fileId).then((details) => {
    // Drawn again for another file meanwhile (the editor reuses its element): not this answer's.
    if (details !== undefined && element.getAttribute(FILE_ID_ATTRIBUTE) === fileId) {
      draw(element, fileId, details);
    }
  });
}

/** Draws every file block under `root` that has not been drawn. */
export function drawFileBlocks(root: HTMLElement): void {
  for (const element of root.querySelectorAll<HTMLElement>(FILE_BLOCK_SELECTOR)) {
    if (!element.hasAttribute(DRAWN)) drawFileBlock(element);
  }
}

/** Saves the file of a drawn block, as a press of its link does; nothing for a missing file. */
export function saveFileOf(element: HTMLElement): boolean {
  const link = element.querySelector<HTMLAnchorElement>('a.doc-file-download');
  link?.click();
  return link !== null;
}

/**
 * Presses a drawn block as Enter does: opens its file in the viewer where
 * its name opens it, saves it otherwise; nothing for a missing file.
 */
export function pressFileOf(element: HTMLElement): boolean {
  const open = element.querySelector<HTMLButtonElement>('button.doc-file-open');
  if (!open) return saveFileOf(element);
  pressedFromBlock = true;
  try {
    open.click();
  } finally {
    pressedFromBlock = false;
  }
  return true;
}
