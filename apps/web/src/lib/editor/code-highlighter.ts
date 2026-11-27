/**
 * Colours for code blocks, as the editor and the read-only view ask for them.
 *
 * The highlighter runs in a worker (code-highlight-worker.ts), started the
 * first time a block in a known language is shown; neither it nor its
 * grammars are part of a screen until then. Texts are coloured one at a
 * time. Each has `HIGHLIGHT_TIME` once the worker is ready, which it says
 * once it is loaded, however long that took: a text that takes
 * longer stops the worker (a new one starts for the next text) and stays
 * plain, and so does every text as long or longer in the same grammar from
 * then on, so that such a text is not tried again with every key typed. A
 * text longer than `CODE_HIGHLIGHT_LIMIT` is not tried at all: drawing that
 * many runs is what would hold up the page then.
 *
 * What came back is remembered by grammar and text, so that a block shown
 * again, or a block that changes back, is coloured at once
 * (`knownTokens`). A worker that cannot start leaves every block plain.
 */
import type { CodeGrammar, CodeLanguage } from './code-languages.ts';
import { HIGHLIGHT_READY, isCodeTokens, type CodeTokens } from './code-tokens.ts';

/** How long the worker may take over one text, in milliseconds, once it is ready. */
export const HIGHLIGHT_TIME = 1000;

/**
 * The longest text that is coloured, in UTF-16 code units: about 130 to 250
 * lines of code. Set by measuring typing in the editor in Chromium. Each run
 * of a colour cuts the block's text, and the editor's drawing of a cut costs
 * about as much as the text after it (ProseMirror's `TextNode.cut`): at 7,500
 * letters a key's redrawing took 7 ms, at 15,000 17 ms, at 37,500 70 ms,
 * where a plain block of any of these took 1 ms. Past this the block is
 * plain, in the editor and where it is only shown alike.
 */
export const CODE_HIGHLIGHT_LIMIT = 10_000;

/** What colours a text. The app's runs in a worker; a test may give one of its own. */
export interface Highlighter {
  /** Settles once the highlighter can take a text; false if it never will. */
  ready(): Promise<boolean>;
  /** The tokens of `text` in `grammar`, or `null`. */
  highlight(grammar: CodeGrammar, text: string): Promise<unknown>;
  /** Stops whatever is under way, which then settles as `null`. */
  stop(): void;
}

type Answer = (tokens: unknown) => void;

/** The highlighter in a worker of the app's own. */
class WorkerHighlighter implements Highlighter {
  #worker: Promise<Worker | undefined> | undefined;
  readonly #waiting = new Map<number, Answer>();
  #last = 0;
  #broken = false;

  #start(): Promise<Worker | undefined> {
    this.#worker ??= import('./code-highlight-worker.ts?worker').then(
      ({ default: HighlightWorker }) => this.#started(HighlightWorker),
      () => {
        this.#broken = true;
        return undefined;
      },
    );
    return this.#worker;
  }

  /** Starts a worker; settles once it says it is ready, or as `undefined` if it never will be. */
  #started(HighlightWorker: new () => Worker): Promise<Worker | undefined> {
    let worker: Worker;
    try {
      worker = new HighlightWorker();
    } catch {
      // No workers here (a test's simulated page), or not allowed.
      this.#broken = true;
      return Promise.resolve(undefined);
    }
    return new Promise((ready) => {
      worker.addEventListener('message', (event: MessageEvent<unknown>) => {
        if (event.data === HIGHLIGHT_READY) {
          ready(worker);
          return;
        }
        const { id, tokens } = event.data as { id: number; tokens: unknown };
        const answer = this.#waiting.get(id);
        this.#waiting.delete(id);
        answer?.(tokens);
      });
      // A worker that fails to load or to run takes its texts with it, and no other starts.
      // Stopped here: failed before it was ready, it is no worker that `stop` knows of.
      worker.addEventListener('error', () => {
        worker.terminate();
        this.#broken = true;
        ready(undefined);
        this.stop();
      });
    });
  }

  async ready(): Promise<boolean> {
    if (this.#broken) return false;
    return (await this.#start()) !== undefined && !this.#broken;
  }

  async highlight(grammar: CodeGrammar, text: string): Promise<unknown> {
    const worker = await this.#start();
    if (!worker) return null;
    const id = (this.#last += 1);
    return new Promise((resolve) => {
      this.#waiting.set(id, resolve);
      worker.postMessage({ id, grammar, text });
    });
  }

  stop(): void {
    const worker = this.#worker;
    this.#worker = undefined;
    void worker?.then((started) => started?.terminate());
    for (const answer of this.#waiting.values()) answer(null);
    this.#waiting.clear();
  }
}

let highlighter: Highlighter | undefined;

/** Puts another highlighter in place of the worker's: for tests. `undefined` goes back to the worker. */
export function useHighlighter(given: Highlighter | undefined): void {
  highlighter?.stop();
  highlighter = given;
  forgetTokens();
}

/** What a request waits for; it gives up when every one that asked has. */
interface Job {
  key: string;
  grammar: CodeGrammar;
  text: string;
  waiting: { resolve: (tokens: CodeTokens | null) => void; signal: AbortSignal | undefined }[];
}

const queue: Job[] = [];
const underWay = new Map<string, Job>();
let pumping = false;

/** What came back for a grammar and a text, newest last. */
const known = new Map<string, CodeTokens | null>();
/** How many code units the remembered texts hold together, and at most. */
let knownSize = 0;
const KNOWN_SIZE = 4_000_000;
const KNOWN_COUNT = 400;

/** The length from which a grammar's texts are not tried again, once one of it took too long. */
const slowFrom = new Map<CodeGrammar, number>();

/** Forgets every answer and every text that took too long. For tests. */
export function forgetTokens(): void {
  known.clear();
  knownSize = 0;
  slowFrom.clear();
}

const keyOf = (grammar: CodeGrammar, text: string): string => `${grammar}\n${text}`;

function remember(key: string, tokens: CodeTokens | null): void {
  known.delete(key);
  known.set(key, tokens);
  knownSize += key.length;
  for (const [oldest] of known) {
    if (knownSize <= KNOWN_SIZE && known.size <= KNOWN_COUNT) break;
    known.delete(oldest);
    knownSize -= oldest.length;
  }
}

/** Whether a text is left plain without asking: too long, or as long as one that took too long. */
function leftPlain(grammar: CodeGrammar, text: string): boolean {
  if (text.length > CODE_HIGHLIGHT_LIMIT) return true;
  const slow = slowFrom.get(grammar);
  return slow !== undefined && text.length >= slow;
}

/**
 * The tokens of a text, if they are known now: `null` for a text that stays
 * plain, `undefined` for one that has to be asked for (`tokensFor`).
 */
export function knownTokens(language: CodeLanguage, text: string): CodeTokens | null | undefined {
  if (text === '' || leftPlain(language.grammar, text)) return null;
  return known.get(keyOf(language.grammar, text));
}

/** Colours one text, in the time it has. */
async function run(job: Job): Promise<CodeTokens | null> {
  highlighter ??= new WorkerHighlighter();
  const working = highlighter;
  if (!(await working.ready())) return null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<'late'>((resolve) => {
    timer = setTimeout(() => {
      resolve('late');
    }, HIGHLIGHT_TIME);
  });
  const answer = await Promise.race([working.highlight(job.grammar, job.text), late]);
  clearTimeout(timer);
  if (answer === 'late') {
    working.stop();
    const slow = slowFrom.get(job.grammar);
    slowFrom.set(job.grammar, Math.min(slow ?? job.text.length, job.text.length));
    return null;
  }
  return isCodeTokens(answer, job.text.length) ? answer : null;
}

async function pump(): Promise<void> {
  if (pumping) return;
  pumping = true;
  try {
    for (let job = queue.shift(); job; job = queue.shift()) {
      const wanted = job.waiting.some(({ signal }) => !signal?.aborted);
      // Nobody waits any more (the text changed again meanwhile): not coloured, not remembered.
      const tokens = wanted && !leftPlain(job.grammar, job.text) ? await run(job) : null;
      underWay.delete(job.key);
      if (wanted) remember(job.key, tokens);
      for (const { resolve } of job.waiting) resolve(tokens);
    }
  } finally {
    pumping = false;
  }
}

/**
 * The tokens of a text: known at once, or once the highlighter has coloured
 * it. `null` when it stays plain. With `signal`, the request is dropped if it
 * has not started by the time the signal is given, and settles as `null`.
 */
export function tokensFor(
  language: CodeLanguage,
  text: string,
  signal?: AbortSignal,
): Promise<CodeTokens | null> {
  const now = knownTokens(language, text);
  if (now !== undefined) return Promise.resolve(now);
  const key = keyOf(language.grammar, text);
  return new Promise((resolve) => {
    const waiting = { resolve, signal };
    let job = underWay.get(key);
    if (job) job.waiting.push(waiting);
    else {
      job = { key, grammar: language.grammar, text, waiting: [waiting] };
      underWay.set(key, job);
      queue.push(job);
    }
    const asked = job;
    signal?.addEventListener(
      'abort',
      () => {
        dropUnwanted(asked);
      },
      { once: true },
    );
    void pump();
  });
}

/**
 * Takes a job nobody waits for any more out of the queue, if it has not
 * started, and settles it: at once, and not when the texts before it are
 * done, which may be never while the worker does not say it is ready.
 */
function dropUnwanted(job: Job): void {
  if (job.waiting.some(({ signal }) => !signal?.aborted)) return;
  const at = queue.indexOf(job);
  if (at === -1) return;
  queue.splice(at, 1);
  underWay.delete(job.key);
  for (const { resolve } of job.waiting) resolve(null);
}
