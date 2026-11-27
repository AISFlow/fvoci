/**
 * The program of the editor's frame (`static/editor-frame.html`).
 *
 * It runs as the viewer's frame does: in a document without an origin
 * (`sandbox="allow-scripts"`, and the server sends the same sandbox in a
 * header) whose policy keeps it off the network. It waits for its parent to
 * hand it a document, has the viewer's worker check the bytes as the viewer
 * checks them, and only then starts the editor, rhwp-studio (the package
 * `@fvoci/rhwp-studio`, built into this frame's bundle), on the checked bytes.
 *
 * The editor is rhwp's own program, written for a page of its own. Here it
 * gets its page's markup put into this document, its settings from a store
 * in memory (the frame has no storage), the engine's bytes as the app handed
 * them over (rhwp.ts), and the host's profile (`?chrome=embed` in the frame's address):
 * it opens, saves and prints nothing itself. Saving is the app's: when asked,
 * the frame answers with the document's bytes together with rhwp's report of
 * what they lose, in one message (`$lib/file-viewer/editor-protocol.ts`).
 *
 * It reads messages from its parent only, and only in the protocol's shapes.
 */
import './frame.css';

import {
  readToEditor,
  type EditFailure,
  type EditFormat,
  type FromEditor,
  type Recount,
  type ToEditor,
} from '$lib/file-viewer/editor-protocol.ts';

import { tallyControls, type ControlTally } from '../viewer-frame/hwp-controls.ts';
import type { FromWorker, ToWorker } from '../viewer-frame/hwp-messages.ts';
import HwpWorker from '../viewer-frame/hwp-worker.ts?worker&inline';
import { WorkerClient } from '../viewer-frame/worker-client.ts';

import { leftOut, openedOf, type Opened } from './comparison.ts';
import { lossesOf } from './losses.ts';
import { withPartsLeftOut } from './parts.ts';
import { notAccepted, recountsOf, type Resaved } from './recounts.ts';
import { provide } from './rhwp.ts';
import { memoryStorage } from './storage.ts';

/** The Korean family the documents fall back to, as in the viewer's frame. */
import.meta.glob('../styles/fonts/faces-noto-sans-kr.css', { eager: true });

/** How long the check of the bytes may take, in milliseconds: the viewer's time for its worker. */
const CHECK_LIMIT = 30_000;
/** How often input (keys, the pointer, the wheel, changes) is told at most, in milliseconds. */
const INPUT_EVERY = 1_000;

/** What the editor's page exposes for its host (our change in rhwp-studio's main.ts). */
interface Host {
  ready(): Promise<void>;
  load(data: Uint8Array, fileName: string): Promise<{ pageCount: number }>;
  exportWithReport(format: EditFormat): { bytes: Uint8Array; contentLoss: unknown };
  /** rhwp's list of the document's controls, as it is now. */
  controls(): unknown;
  onDirtyChanged(listener: (dirty: boolean) => void): () => void;
  /** Called on every change to the document. */
  onMutated(listener: () => void): () => void;
  /** Puts the focus where the document is typed into. */
  focus(): void;
  notifySaved(): Promise<unknown>;
}

/** The editor's page: its hook for the host, and its answer to links that lead out of a document. */
interface Studio {
  host?: Host;
  links?: { setHandler(handler: () => boolean): void };
}

function tell(message: FromEditor, transfer: Transferable[] = []): void {
  // The parent's origin is not known here (ours is opaque); the parent checks
  // that a message came from this frame.
  window.parent.postMessage(message, '*', transfer);
}

/** The bytes as rhwp may read them, checked in the viewer's worker; the reason when they are refused. */
async function checked(format: EditFormat, bytes: ArrayBuffer): Promise<ArrayBuffer | EditFailure> {
  const client = new WorkerClient(new HwpWorker(), CHECK_LIMIT);
  try {
    const message: ToWorker = { kind: 'check', format, bytes };
    const answer = (await client.request(message, [bytes])) as FromWorker;
    if (answer.kind === 'checked') return answer.bytes;
    return answer.kind === 'failed' && answer.reason !== 'renderer' ? answer.reason : 'editor';
  } catch {
    return 'editor';
  } finally {
    client.close();
  }
}

/** The markup of the editor's page, as its own document has it in its body; its script left out. */
function pageMarkup(page: string): string {
  const body = /<body[^>]*>([\s\S]*)<\/body>/i.exec(page)?.[1];
  if (body === undefined) throw new Error("the editor's page has no body");
  return body.replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, '');
}

/**
 * Starts the editor in this document: its settings (the app's theme and
 * language) in a store in memory, its page's markup, its style sheet, and
 * then its program, which finds the page as its own document would give it.
 */
async function startEditor(theme: string, lang: string): Promise<Host> {
  const settings = memoryStorage({
    'rhwp-settings': JSON.stringify({ theme: { mode: theme } }),
    'rhwp-locale': lang.toLowerCase().startsWith('ko') ? 'ko' : 'en',
  });
  // The frame has no storage (reading `localStorage` throws) and no database.
  for (const name of ['localStorage', 'sessionStorage'] as const) {
    Object.defineProperty(window, name, { configurable: true, value: settings });
  }
  const [{ default: page }] = await Promise.all([
    import('rhwp-studio:page'),
    import('rhwp-studio:style'),
  ]);
  document.body.innerHTML = pageMarkup(page);
  await import('rhwp-studio:theme-init');
  await import('rhwp-studio:locale-init');
  await import('rhwp-studio:main');
  const studio = (window as unknown as { rhwpStudio?: Studio }).rhwpStudio;
  const host = studio?.host;
  if (!host || !studio.links) throw new Error('the editor has no hook for its host');
  // A link to another file, a program or an address of mail leads nowhere from here: the
  // editor would ask for a file to open in the document's place.
  studio.links.setHandler(() => true);
  await host.ready();
  return host;
}

/**
 * `promise`'s value, or undefined where it takes past `deadline` (by
 * `performance.now()`) or fails.
 */
async function until<T>(promise: Promise<T>, deadline: number): Promise<T | undefined> {
  let timer: number | undefined;
  const late = new Promise<undefined>((resolve) => {
    timer = window.setTimeout(resolve, Math.max(0, deadline - performance.now()));
  });
  try {
    return await Promise.race([promise.catch(() => undefined), late]);
  } finally {
    clearTimeout(timer);
  }
}

/**
 * The bytes to save, read back as the viewer reads a file: checked as it
 * checks one, and opened by rhwp in a worker of their own, apart from the
 * document being edited (by `deadline` at the latest, as before the counts);
 * then its controls counted, as the document's in the editor are
 * (recounts.ts), unless the time is out by then. Undefined where they do not
 * open: what does not is not handed to the app.
 */
async function readBack(
  format: EditFormat,
  bytes: ArrayBuffer,
  engine: ArrayBuffer,
  deadline: number,
): Promise<{ controls: ControlTally | undefined } | undefined> {
  let client: WorkerClient | undefined;
  try {
    client = new WorkerClient(new HwpWorker(), Math.max(0, deadline - performance.now()));
    const message: ToWorker = { kind: 'open', format, bytes, wasm: engine };
    const opened = (await client.request(message, [bytes, engine])) as FromWorker;
    if (opened.kind !== 'opened') return undefined;
    const asked = client.request({ kind: 'controls' } satisfies ToWorker);
    const counted = (await until(asked, deadline)) as FromWorker | undefined;
    return { controls: counted?.kind === 'controls' ? counted.controls : undefined };
  } catch {
    return undefined;
  } finally {
    client?.close();
  }
}

/**
 * The pages the file given lays out on, and the pages it lays out on saved
 * unchanged and read back, both in a worker of their own as the viewer lays a
 * file out (recounts.ts). Undefined where it could not be counted: then no
 * page is asked about, and the controls are still.
 */
async function resaved(
  format: EditFormat,
  bytes: ArrayBuffer,
  engine: ArrayBuffer,
): Promise<Resaved | undefined> {
  let client: WorkerClient | undefined;
  try {
    client = new WorkerClient(new HwpWorker(), CHECK_LIMIT);
    const message: ToWorker = { kind: 'open', format, bytes, wasm: engine };
    const opened = (await client.request(message, [bytes, engine])) as FromWorker;
    if (opened.kind !== 'opened') return undefined;
    const counted = (await client.request({ kind: 'resaved' } satisfies ToWorker)) as FromWorker;
    return counted.kind === 'resaved' ? [counted.pages, counted.saved] : undefined;
  } catch {
    return undefined;
  } finally {
    client?.close();
  }
}

/**
 * The keys the app answers, told it: Ctrl+S (Cmd+S) to save, which the
 * editor's host profile swallows itself; F6 to leave the document for the
 * app's controls, the editor keeping Tab and Escape to itself.
 */
function answerKeys(): void {
  document.addEventListener(
    'keydown',
    (event) => {
      const key = event.key.toLowerCase();
      const plain = !event.altKey && !event.shiftKey;
      if ((event.ctrlKey || event.metaKey) && plain) {
        if (key === 's' || key === 'ㄴ') tell({ kind: 'save-asked' });
      } else if (event.key === 'F6' && plain) {
        event.preventDefault();
        event.stopImmediatePropagation();
        tell({ kind: 'leave' });
      }
    },
    true,
  );
}

/** The editor at work on one document: answers the app's requests to export and to mark it stored. */
function serve(
  host: Host,
  format: EditFormat,
  engine: ArrayBuffer,
  given: Opened,
  pages: Promise<Resaved | undefined>,
): (message: ToEditor) => void {
  let opened = given;
  host.onDirtyChanged((dirty) => {
    tell({ kind: 'changed', dirty });
  });
  // How many changes the document has had, and how many it had at each export: what was
  // exported is the document only while nothing changed since.
  let changes = 0;
  const exportedAt = new Map<number, number>();
  // The writer's input, which the app does not see in here, is told: the file's lease is let
  // go of when there is none for a while (lease.svelte.ts). Keys, the pointer, the wheel and
  // changes to the document, at most once in `INPUT_EVERY` (a replacement of every match is
  // many changes at once), by the events' own time.
  let inputAt = -Infinity;
  const input = (at: number): void => {
    if (at - inputAt < INPUT_EVERY) return;
    inputAt = at;
    tell({ kind: 'input' });
  };
  const told = (event: Event): void => {
    input(event.timeStamp);
  };
  for (const type of ['keydown', 'pointerdown', 'wheel'] as const) {
    document.addEventListener(type, told, { capture: true, passive: true });
  }
  host.onMutated(() => {
    changes += 1;
    input(performance.now());
  });
  // When the app gives the frame the focus (as the document opens, and after a save asked with
  // the app's button), what is typed goes to the document again. WebKit lets a frame of another
  // origin move its focus only in answer to the person: the frame has the focus, its document's
  // field not yet. A key pressed then gives the field the focus, and what it types goes there.
  const toDocument = (): void => {
    if (document.activeElement === document.body) host.focus();
  };
  window.addEventListener('focus', toDocument);
  if (document.hasFocus()) toDocument();
  document.addEventListener('keydown', toDocument, true);
  answerKeys();
  // What is kept of the last export: once stored as the file's new version, it is what later
  // saves are compared with (the file is then what was stored, not what was opened), and the
  // counts it read back with otherwise are ones the writer chose to save with.
  let last: { request: number; kept: Opened; recounts: Recount[] } | undefined;
  let accepted: Recount[] = [];
  const exportFor = async (request: number): Promise<void> => {
    try {
      const exported = host.exportWithReport(format);
      // The document's controls as it is exported; not counted where rhwp cannot list them, and
      // then not asked about, as where the saved file's are not counted.
      let before: ControlTally | undefined;
      try {
        before = tallyControls(host.controls());
      } catch {
        before = undefined;
      }
      const report = lossesOf(exported.contentLoss);
      // A comparison that cannot be made is an export that failed: nothing is saved unasked.
      const left = leftOut(opened, exported.bytes);
      const { count, losses } = withPartsLeftOut(report, left.losses);
      const bytes = exported.bytes.slice().buffer;
      const at = changes;
      // The reading back and the counts take the time the reading back took alone: the app's
      // time for an export is not passed. A count not made by then is not asked about.
      const deadline = performance.now() + CHECK_LIMIT;
      const read = await readBack(format, bytes.slice(0), engine.slice(0), deadline);
      if (!read) throw new Error('not opened');
      exportedAt.set(request, at);
      const counted = recountsOf(await until(pages, deadline), before, read.controls);
      const recounts = notAccepted(counted, accepted);
      last = { request, kept: left.kept, recounts };
      const { unprotected } = left;
      tell({ kind: 'exported', request, bytes, count, losses, unprotected, recounts }, [bytes]);
    } catch {
      tell({ kind: 'export-failed', request });
    }
  };
  return (message) => {
    if (message.kind === 'export') {
      void exportFor(message.request);
    } else if (message.kind === 'stored') {
      if (message.as === 'version' && last?.request === message.request) {
        opened = last.kept;
        accepted = [...accepted, ...last.recounts];
      }
      last = undefined;
      // Only what was exported is stored: a change made since keeps the document changed.
      if (exportedAt.get(message.request) === changes)
        void host.notifySaved().catch(() => undefined);
    }
  };
}

/** Keeps a document's links and dropped files from taking the frame away or into the editor. */
function holdNavigation(): void {
  const holdLink = (event: MouseEvent): void => {
    const link = event.target instanceof Element ? event.target.closest('a, area') : null;
    if (link && !(link.getAttribute('href') ?? '').startsWith('#')) event.preventDefault();
  };
  document.addEventListener('click', holdLink, true);
  document.addEventListener('auxclick', holdLink, true);
  // A file dropped on the editor would be opened in place of the document being edited (which
  // the editor refuses once it has the app's document: our change in its main.ts).
  for (const type of ['dragover', 'drop'] as const) {
    document.addEventListener(
      type,
      (event) => {
        if (!event.dataTransfer?.types.includes('Files')) return;
        event.preventDefault();
        event.stopImmediatePropagation();
      },
      true,
    );
  }
}

function listen(): void {
  let serving: ((message: ToEditor) => void) | undefined;
  let opening = false;
  window.addEventListener('message', (event) => {
    if (event.source !== window.parent) return;
    const message = readToEditor(event.data);
    if (!message) return;
    if (message.kind !== 'open') {
      serving?.(message);
      return;
    }
    if (opening) return;
    opening = true;
    const { format, name, bytes, engine, theme, lang } = message;
    void (async () => {
      const file = await checked(format, bytes);
      if (typeof file === 'string') {
        tell({ kind: 'failed', reason: file });
        return;
      }
      try {
        // The loader reads the bytes and leaves them whole: saves are read back with them.
        provide(engine);
        // The check writes the file anew with every part and stream it had.
        const opened = openedOf(format, new Uint8Array(file));
        // Counted while the editor starts: a save that comes first waits for it.
        const pages = resaved(format, file.slice(0), engine.slice(0));
        const host = await startEditor(theme, lang);
        const { pageCount } = await host.load(new Uint8Array(file), name);
        serving = serve(host, format, engine, opened, pages);
        tell({ kind: 'opened', pages: pageCount });
      } catch {
        tell({ kind: 'failed', reason: 'editor' });
      }
    })();
  });
  tell({ kind: 'ready' });
}

// Only as somebody's frame, in a document without an origin. Opened as a page of its own it
// has nobody to hear (the server's sandbox leaves it without an origin there too, so nothing
// of the app's session is in its reach), and with the app's origin the editor would run
// beside the app's session.
if (self.origin === 'null' && window.parent !== window) {
  holdNavigation();
  listen();
}
