/**
 * The program of the viewer's frame (`static/viewer-frame.html`).
 *
 * It runs in a document without an origin (`sandbox="allow-scripts"`, and
 * the server sends the same sandbox in a header) whose policy keeps it off
 * the network. It waits for its parent to hand it a file, draws it with the
 * renderer of its kind, and reports back through the messages of
 * `$lib/file-viewer/protocol.ts`. It reads messages from its parent only, and
 * only in those shapes.
 */
import './frame.css';

import { isPackaged } from '$lib/file-viewer/formats.ts';
import {
  readToFrame,
  VIEWER_KEYS,
  type Failure,
  type FromFrame,
  type Theme,
  type ViewerKey,
} from '$lib/file-viewer/protocol.ts';

import {
  LockedFile,
  SlowRenderer,
  TooLargeFile,
  UnreadableFile,
  type Drawn,
  type Report,
} from './renderer.ts';
import { CHECKED_IN_WORKER, RENDERERS } from './renderers.ts';
import { repack } from './zip-guard.ts';

/**
 * The font rules the documents fall back to: the Korean family, whose files
 * draw Latin text as well (lib/fonts.ts loads the same rules in the app). Not
 * the Latin family's: the app's main style sheet has those, and a style sheet
 * both pages took would be split out of it.
 */
import.meta.glob('../styles/fonts/faces-noto-sans-kr.css', { eager: true });

function tell(message: FromFrame): void {
  // The parent's origin is not known here (ours is opaque), and nothing sent
  // is secret: the parent checks that a message came from this frame.
  window.parent.postMessage(message, '*');
}

function setTheme(theme: Theme): void {
  document.documentElement.classList.toggle('dark', theme === 'dark');
  document.documentElement.style.colorScheme = theme;
}

const keys = new Set<string>(VIEWER_KEYS);

/**
 * Keeps a document's links from taking the frame away: a link a document
 * carries would load another page in the frame, with scripts and the network,
 * inside the app's dialog. Only links to a place in the document itself go.
 */
function holdLinks(): void {
  const hold = (event: MouseEvent): void => {
    const link = event.target instanceof Element ? event.target.closest('a, area') : null;
    if (!link) return;
    const target = link.getAttribute('href') ?? link.getAttribute('xlink:href') ?? '';
    if (!target.startsWith('#')) event.preventDefault();
  };
  document.addEventListener('click', hold, true);
  document.addEventListener('auxclick', hold, true);
}

function forwardKeys(): void {
  document.addEventListener('keydown', (event) => {
    if (event.ctrlKey || event.metaKey || event.altKey || !keys.has(event.key)) return;
    event.preventDefault();
    // With Shift, the arrows across scroll the page across, as the arrows down scroll it: a
    // sheet or a page wider than the view is read sideways so. Without, they turn pages.
    if (event.shiftKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
      const step = Math.round(Math.max(40, window.innerWidth / 4));
      window.scrollBy({ left: event.key === 'ArrowLeft' ? -step : step });
      return;
    }
    tell({ kind: 'key', key: event.key as ViewerKey });
  });
}

const report: Report = {
  sheets: (sheets) => {
    tell({ kind: 'sheets', sheets });
  },
  hidden: (pages) => {
    tell({ kind: 'hidden', pages });
  },
  shown: (pages) => {
    tell({ kind: 'shown', pages });
  },
  page: (page) => {
    tell({ kind: 'page', page });
  },
  failed: (reason) => {
    tell({ kind: 'failed', reason });
  },
  download: () => {
    tell({ kind: 'download' });
  },
};

function failureOf(error: unknown): Failure {
  if (error instanceof LockedFile) return 'locked';
  if (error instanceof UnreadableFile) return 'unreadable';
  if (error instanceof TooLargeFile) return 'too-large';
  return error instanceof SlowRenderer ? 'slow' : 'renderer';
}

function start(): void {
  const place = document.getElementById('document');
  if (!place) return;
  let drawn: Drawn | undefined;
  let opened = false;

  window.addEventListener('message', (event) => {
    if (event.source !== window.parent) return;
    const message = readToFrame(event.data);
    if (!message) return;
    switch (message.kind) {
      case 'open': {
        // One file per frame: the viewer makes a new frame for the next one.
        if (opened) return;
        opened = true;
        setTheme(message.theme);
        // The language of the app: what a screen reader reads the document's text in.
        document.documentElement.lang = message.lang;
        RENDERERS[message.format](message.engine)
          .then(async (render) => {
            // An archive's renderer gets it anew, of only the parts counted here (or in its worker).
            const here = isPackaged(message.format) && !CHECKED_IN_WORKER.has(message.format);
            const bytes = here ? await repack(message.bytes) : message.bytes;
            return render(bytes, place, report, message.labels);
          })
          .then((result) => {
            drawn = result;
          })
          .catch((error: unknown) => {
            place.replaceChildren();
            tell({ kind: 'failed', reason: failureOf(error) });
          });
        break;
      }
      case 'page':
        drawn?.goTo(message.page);
        break;
      case 'zoom':
        drawn?.zoom(message.zoom);
        break;
      case 'theme':
        setTheme(message.theme);
        break;
      case 'hidden':
        drawn?.showHidden?.(message.shown);
        break;
    }
  });
  forwardKeys();
  holdLinks();
  tell({ kind: 'ready' });
}

// A document of the app's own origin would be opened as a page of the app,
// its storage and cookies within reach: this program runs sandboxed or not at all.
if (self.origin === 'null') start();
