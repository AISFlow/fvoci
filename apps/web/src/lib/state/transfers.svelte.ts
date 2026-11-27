/**
 * Everything the reader watches go between the page and the server, in one
 * stack at the bottom right of the page (features/shell/Transfers.svelte):
 * the archives exported and imported (`Workspace.exportContainer`,
 * `exportWorkspace`, `importArchive`), and every file uploaded (`upload`: an
 * attachment of a card, an image or a file block of a body, a new version of
 * a file).
 *
 * Each shows its progress, and stays a moment after it is done; that moment
 * waits while the reader points at the stack or is in it (`holdWhile`). A failed
 * upload stays, with why, until it is dismissed, and one that can be sent
 * again says so (`retry`); a failed archive is told where problems are. One
 * dismissed while under way is given up and told nothing, unless what it did
 * by then could not be taken back (`UploadKept`): it comes back to say so.
 * Each belongs to the workspace it was started in and is shown only there; it
 * goes on meanwhile.
 *
 * Only a start, an end and a failure are said (`said`), each in its turn, so
 * that two at once are both heard: not the counts that change with every part.
 */
import { isBackendError } from './backend.ts';
import type { ProblemAction } from './workspace.svelte.ts';

/** Why an upload failed, for the words that say it (`failureOf`). */
export type UploadFailure =
  /** Larger than the server takes (413; `limit` says how large it may be). */
  | 'too-large'
  /** The server takes no file of this name (400). */
  | 'refused'
  /** No answer: the connection dropped. */
  | 'network'
  /** No longer the reader's to add to (403), or no longer there (404). */
  | 'forbidden'
  /** The reader has as many uploads open as one may (429 `too_many_uploads`). */
  | 'busy'
  /** The uploads open hold all the room the server keeps for them (429). */
  | 'room'
  /** What it was going up for closed before it was in (a body's image or file). */
  | 'left'
  /** Given up once it was in already (attached), and not taken out again: it stays there. */
  | 'kept'
  | 'failed';

/** Something on its way down or up, for the reader to watch. */
export interface Transfer {
  readonly id: number;
  readonly kind: 'export' | 'import' | 'upload';
  /** What is moved: the board's title, the workspace's, or the file's name. */
  readonly name: string;
  readonly done: number;
  /** 0 while the length is not known. */
  readonly total: number;
  readonly finished: boolean;
  /** Why an upload failed: it stays until dismissed. */
  readonly failure?: UploadFailure;
  /** With `too-large`, the largest file the server takes, when it said. */
  readonly limit?: number;
  /** Sends a failed upload again, where that can be done. */
  readonly retry?: () => void;
  /** The workspace it was started in: it is shown there only. */
  readonly workspaceId?: string;
  /** The card a file is attached to: its list of attachments shows it in place. */
  readonly card?: string;
}

/** One start, end or failure, as it was when it was said; `key` tells each from the others. */
export interface Saying {
  readonly key: number;
  readonly transfer: Transfer;
}

/**
 * What an upload's `send` throws when it was given up only once it had done
 * its work (a file attached), and that could not be undone: the upload comes
 * back to say it stays where it went.
 */
export class UploadKept extends Error {
  constructor(cause: unknown) {
    super('given up once it was in, and not taken out again', { cause });
    this.name = 'UploadKept';
  }
}

/** What a transfer is given to report how far it is, and to be given up by. */
export interface TransferOptions {
  onProgress: (progress: { sentBytes: number; totalBytes: number }) => void;
  signal: AbortSignal;
}

/** A file to upload (`Transfers.upload`). */
export interface UploadRequest<T> {
  readonly file: File;
  readonly card?: string;
  /** Uploads it, and does whatever comes of it (attaches it): all of it again on `retry`. */
  readonly send: (options: TransferOptions) => Promise<T>;
  /**
   * Whether a failure can be sent again: not where its place is gone with it (a
   * body's). Even so, only one that sending again may get past is offered it.
   */
  readonly retryable?: boolean;
}

/** What the stack is of (the `Workspace`): what it tells there, and the workspace shown. */
export interface TransferShell {
  /** A failed archive, as a problem of its kind. */
  report(kind: ProblemAction, error: unknown): void;
  /** Whether a failed upload's error ended the session or closed the workspace (done then). */
  ends(error: unknown): boolean;
  /** The workspace shown: what starts now is of it. */
  readonly info: { readonly id: string } | undefined;
}

/**
 * The failures that sending the same file again may get past: the connection,
 * the uploads open, something the server did not say. Not a file too large, a
 * name refused, or a card that is no longer the reader's.
 */
const PASSING: readonly UploadFailure[] = ['network', 'busy', 'room', 'failed'];

/** How long a finished transfer stays in sight, in milliseconds. */
export const TRANSFER_STAYS = 6000;

/**
 * How many of the latest sayings are kept for the stack to read. More said at
 * once, as many files dropped together start, are not all read: the last are.
 */
const SAYINGS_KEPT = 20;

/** Why an upload failed, from what it failed with. */
export function failureOf(error: unknown): { failure: UploadFailure; limit?: number } {
  if (!isBackendError(error)) return { failure: 'failed' };
  switch (error.status) {
    case 0:
      return { failure: 'network' };
    case 400:
      return { failure: 'refused' };
    case 403:
    case 404:
      return { failure: 'forbidden' };
    case 413:
      return error.limit ? { failure: 'too-large', limit: error.limit } : { failure: 'too-large' };
    case 429:
      return { failure: error.reason === 'too_many_uploads' ? 'busy' : 'room' };
    default:
      return { failure: 'failed' };
  }
}

export class Transfers {
  list = $state.raw<Transfer[]>([]);
  /** The latest starts, ends and failures, oldest first, as each was then: what a screen reader is told. */
  said = $state.raw<readonly Saying[]>([]);
  readonly #shell: TransferShell;
  #count = 0;
  #sayings = 0;
  /** What gives up each transfer under way. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #running = new Map<number, AbortController>();
  /** What each export or import moves: the same is not moved twice at once. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #keys = new Map<number, string>();
  /** The timers of those finished, until they go. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #going = new Map<number, ReturnType<typeof setTimeout>>();
  /** Whether the stack is held as it is (`holdWhile`): asked when a finished one would go. */
  #held: () => boolean = () => false;

  /** `shell` is asked when a transfer starts, and when one fails. */
  constructor(shell: TransferShell) {
    this.#shell = shell;
  }
  /** Those of the workspace `workspaceId` (and those of none), oldest first. */
  of(workspaceId: string | undefined): Transfer[] {
    return this.list.filter(
      (one) => one.workspaceId === undefined || one.workspaceId === workspaceId,
    );
  }

  /**
   * Runs an export or an import that the reader can watch. A failed one is
   * told as a problem, and goes; one given up is told nothing. The same thing
   * (`key`) is not moved twice at once: asked again meanwhile, nothing more is done.
   */
  async run<T>(
    kind: 'export' | 'import',
    key: string,
    name: string,
    run: (options: TransferOptions) => Promise<T>,
  ): Promise<T | undefined> {
    if ([...this.#keys.values()].includes(key)) return undefined;
    const id = this.#begin({ kind, name, total: 0 });
    this.#keys.set(id, key);
    try {
      return await this.#send(id, run, (error) => {
        this.dismiss(id);
        this.#shell.report(kind, error);
      });
    } finally {
      this.#keys.delete(id);
    }
  }

  /**
   * Uploads a file that the reader can watch: what `send` gives, or undefined
   * when it failed (it stays, with why) or was given up. Its `id` is at once
   * the transfer's.
   */
  upload<T>(request: UploadRequest<T>): { id: number; done: Promise<T | undefined> } {
    const { file, card } = request;
    const id = this.#begin({ kind: 'upload', name: file.name, total: file.size, card });
    return { id, done: this.#sendFile(id, request) };
  }

  /**
   * An upload under way is given up because what it was going up for closed:
   * it stays, failed for that reason, until dismissed.
   */
  leave(id: number): void {
    const stop = this.#running.get(id);
    if (!stop) return;
    this.#change(id, { failure: 'left' });
    stop.abort();
    this.#say(id);
  }

  /** Takes a transfer out of sight; one under way is given up. */
  dismiss(id: number): void {
    this.#running.get(id)?.abort();
    clearTimeout(this.#going.get(id));
    this.#going.delete(id);
    this.list = this.list.filter((one) => one.id !== id);
  }

  /**
   * The finished stay while `held` says the stack is held (the pointer over it,
   * the focus in it): asked when one would go, it then stays another moment.
   * What it gives stops the asking.
   */
  holdWhile(held: () => boolean): () => void {
    this.#held = held;
    return () => {
      if (this.#held === held) this.#held = () => false;
    };
  }

  /** The stack was let go of: the finished stay their whole moment from now. */
  restart(): void {
    for (const one of this.list) if (one.finished) this.#goLater(one.id);
  }

  #begin(fields: Omit<Transfer, 'id' | 'done' | 'finished' | 'workspaceId'>): number {
    this.#count += 1;
    const id = this.#count;
    const workspaceId = this.#shell.info?.id;
    this.list = [...this.list, { id, done: 0, finished: false, workspaceId, ...fields }];
    this.#say(id);
    return id;
  }

  async #sendFile<T>(id: number, request: UploadRequest<T>): Promise<T | undefined> {
    return this.#send(id, request.send, (error) => {
      // A session that ended, or a workspace that closed, is told by the shell itself.
      this.#shell.ends(error);
      if (error !== undefined && !isBackendError(error)) console.error(error);
      const again = (): void => {
        this.#change(id, { failure: undefined, limit: undefined, retry: undefined, done: 0 });
        this.#say(id);
        void this.#sendFile(id, request);
      };
      const why = failureOf(error);
      const passing = request.retryable && PASSING.includes(why.failure);
      this.#change(id, { ...why, retry: passing ? again : undefined });
      this.#say(id);
    });
  }

  /** Runs one transfer: its progress, its end; `failed` is told how it failed, unless given up. */
  async #send<T>(
    id: number,
    run: (options: TransferOptions) => Promise<T>,
    failed: (error: unknown) => void,
  ): Promise<T | undefined> {
    const stop = new AbortController();
    this.#running.set(id, stop);
    const begun = this.list.find((one) => one.id === id);
    try {
      const result = await run({
        onProgress: ({ sentBytes, totalBytes }) => {
          this.#change(id, { done: sentBytes, total: totalBytes });
        },
        signal: stop.signal,
      });
      if (stop.signal.aborted) return undefined;
      this.#change(id, { finished: true });
      this.#say(id);
      this.#goLater(id);
      return result;
    } catch (error) {
      if (!stop.signal.aborted) failed(error);
      else if (error instanceof UploadKept && begun) this.#keep(begun);
      return undefined;
    } finally {
      this.#running.delete(id);
    }
  }

  /** One given up that stays where it went: back in the stack, said, until dismissed. */
  #keep(begun: Transfer): void {
    const kept: Transfer = { ...begun, failure: 'kept', limit: undefined, retry: undefined };
    this.list = [...this.list.filter((one) => one.id !== begun.id), kept];
    this.#say(begun.id);
  }

  #change(id: number, patch: Partial<Omit<Transfer, 'id'>>): void {
    this.list = this.list.map((one) => (one.id === id ? { ...one, ...patch } : one));
  }

  #say(id: number): void {
    const one = this.list.find((each) => each.id === id);
    if (!one) return;
    this.#sayings += 1;
    this.said = [...this.said, { key: this.#sayings, transfer: one }].slice(-SAYINGS_KEPT);
  }

  /** A finished one goes a moment from now, or, the stack held then, a moment after. */
  #goLater(id: number): void {
    clearTimeout(this.#going.get(id));
    this.#going.set(
      id,
      setTimeout(() => {
        if (this.#held()) this.#goLater(id);
        else this.dismiss(id);
      }, TRANSFER_STAYS),
    );
  }
}
