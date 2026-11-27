/**
 * One opening of the search: what is typed, the kind chosen, and what the
 * search answers.
 *
 * It searches as the reader types, once the typing has stopped for a
 * moment (`TYPING_PAUSE`), and again at once when the kind changes. Only the
 * answer to the last request is shown: an earlier one that arrives late is
 * dropped, and so is everything that arrives after the session is closed.
 *
 * Told to wait (429), it says so and asks once more when the server said it
 * may; told again, it says so with the way to try again, as for a search
 * server that does not answer (503). A session that ended (401) is passed on
 * (`onRefused`) as well as said.
 */
import {
  isBackendError,
  type SearchAsk,
  type SearchHit,
  type SearchKind,
} from '$lib/state/backend.ts';

/** How long typing stops before what was typed is searched for, in milliseconds. */
export const TYPING_PAUSE = 250;
/** How long to wait when told to without a time, and the longest wait, in seconds. */
const DEFAULT_WAIT = 1;
const LONGEST_WAIT = 30;
/** How many hits are asked for. */
export const HITS_ASKED = 20;

export type SearchStatus =
  /** Nothing typed. */
  | 'empty'
  /** A search is asked for, or will be once typing stops. */
  | 'searching'
  | 'found'
  /** The search found nothing. */
  | 'none'
  /** Too many searches: it tries again in a moment. */
  | 'limited'
  /** Too many searches still, after trying again once: the reader tries again. */
  | 'stillLimited'
  /** The search server does not answer. */
  | 'unavailable'
  /** Anything else went wrong. */
  | 'failed';

export interface SearchSessionOptions {
  /** Asks the backend. */
  find: (ask: SearchAsk) => Promise<SearchHit[]>;
  /** A refusal that is not the search's alone (a session that ended), for the page to tell. */
  onRefused?: (error: unknown) => void;
}

export class SearchSession {
  query = $state('');
  /** One kind only; both when none. */
  kind = $state<SearchKind | undefined>(undefined);
  status = $state<SearchStatus>('empty');
  /** What the last answer found; kept while the next is on its way. */
  hits = $state.raw<SearchHit[]>([]);

  readonly #options: SearchSessionOptions;
  /** The request whose answer is shown when it comes; a newer one, or a closing, changes it. */
  #turn = 0;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #closed = false;
  /** Who waits for the answer to the request of `#turn` (`answered`). */
  #waiting: ((hits: SearchHit[] | undefined) => void)[] = [];

  constructor(options: SearchSessionOptions) {
    this.#options = options;
  }

  /** What was typed: searched for once typing stops. */
  type(query: string): void {
    this.query = query;
    this.#schedule(TYPING_PAUSE, true);
  }

  /** Another kind is searched for at once. */
  choose(kind: SearchKind | undefined): void {
    this.kind = kind;
    this.#schedule(0, true);
  }

  /** Asks again, at once: after the search server did not answer. */
  retry(): void {
    this.#schedule(0, true);
  }

  /** Nothing more is asked, and no answer on its way is shown. */
  close(): void {
    this.#closed = true;
    this.#turn += 1;
    clearTimeout(this.#timer);
    this.#settle(undefined);
  }

  /**
   * What the search for the words in the field finds, once it answers: the
   * hits shown now where there is no search on its way. Nothing (`undefined`)
   * where the words change before it answers, it is refused, or the session
   * closes: what was waited for is no more.
   */
  answered(): Promise<SearchHit[] | undefined> {
    if (this.status !== 'searching') return Promise.resolve(this.hits);
    return new Promise((resolve) => {
      this.#waiting.push(resolve);
    });
  }

  #settle(hits: SearchHit[] | undefined): void {
    for (const resolve of this.#waiting.splice(0)) resolve(hits);
  }

  /**
   * Asks after `delay`, in place of whatever was to be asked or is on its
   * way. `again` is whether being told to wait may ask once more.
   */
  #schedule(delay: number, again: boolean): void {
    if (this.#closed) return;
    clearTimeout(this.#timer);
    const turn = (this.#turn += 1);
    if (again) this.#settle(undefined);
    if (this.query.trim() === '') {
      this.status = 'empty';
      this.hits = [];
      return;
    }
    if (again) this.status = 'searching';
    this.#timer = setTimeout(() => void this.#ask(turn, again), delay);
  }

  async #ask(turn: number, again: boolean): Promise<void> {
    const ask: SearchAsk = { query: this.query.trim(), kind: this.kind, limit: HITS_ASKED };
    try {
      const hits = await this.#options.find(ask);
      if (turn !== this.#turn) return;
      this.hits = hits;
      this.status = hits.length > 0 ? 'found' : 'none';
      this.#settle(hits);
    } catch (error) {
      if (turn !== this.#turn) return;
      this.#settle(undefined);
      this.#refused(error, again);
    }
  }

  #refused(error: unknown, again: boolean): void {
    if (!isBackendError(error)) {
      this.status = 'failed';
      return;
    }
    if (error.status === 429) {
      this.status = again ? 'limited' : 'stillLimited';
      if (!again) return;
      const seconds = Math.min(LONGEST_WAIT, Math.max(DEFAULT_WAIT, error.retryAfter ?? 0));
      this.#schedule(seconds * 1000, false);
      return;
    }
    this.status = error.status === 503 ? 'unavailable' : 'failed';
    if (error.status === 401) this.#options.onRefused?.(error);
  }
}
