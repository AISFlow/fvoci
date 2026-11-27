/**
 * One editor of a file at a time: the file's lease, which the server keeps
 * (`holdFileLease`). "Edit" takes it; the editor holds on to it every 30
 * seconds while it is open and as soon as its window is shown again, and lets
 * it go when it is closed, or with the page as the page goes (a request that
 * outlives it). A window that dies keeps it only until its term ends (90
 * seconds, the server's): nothing is held for ever.
 *
 * An editor nobody puts anything into for 30 minutes (`IDLE_AFTER`: no key,
 * no pointer, no change to the document) lets its lease go, says so, and
 * keeps what is not saved; the next input takes the lease again. It says so
 * 5 minutes before (`WARN_BEFORE`). The time is the clock's, read at each
 * turn: a window asleep that long lets the lease go as it wakes.
 *
 * A lease keeps nobody from writing a version: a save is held to the version
 * it was made from. A lease lost (a window asleep past its term while
 * somebody else took the file's, or somebody else took it while the editor
 * had let it go) is told to the writer, and the editor goes on.
 *
 * A request to the server not answered within a turn is waited for no longer
 * (a connection half dead): asked again at the next. A letting go given up on
 * so leaves the lease held, by the name the next input holds on to. A holding
 * on given up on may yet have given the editor a lease under a name it does
 * not know: answered late, the lease is the editor's by that name; not
 * answered, the file's lease the reader holds is not told as somebody else's
 * for a term after it, and the lease, held on to by nobody, ends.
 *
 * The viewer asks whether somebody edits the file (`LeaseWatch`) as it opens,
 * every 15 seconds while it is shown, and after a lease was refused.
 */

/** Whether somebody has a file open in the editor, and who. */
export interface FileLease {
  editing: boolean;
  /** Who, where the reader may know (the reader included); empty for nobody, and for somebody else. */
  editorId: string;
}

/** A lease the reader holds. */
export interface HeldFileLease {
  leaseId: string;
  /** How many seconds it lasts unless held on to before then. */
  seconds: number;
}

/** What a file's lease is reached through: the file's versions (versions.svelte.ts). */
export interface LeaseSource {
  /** Who the reader is, where known. */
  readonly me: string | undefined;
  lease(): Promise<FileLease>;
  /** Takes the lease (`leaseId` empty) or holds on to it; `'editing'` while another holds the file's. */
  holdLease(leaseId: string): Promise<HeldFileLease | 'editing'>;
  /** Lets it go; `keepalive` as the page goes. */
  releaseLease(leaseId: string, keepalive: boolean): Promise<void>;
  /** Reads the names of those it names. */
  learnUsers(userIds: string[]): Promise<void>;
}

export interface LeaseTimes {
  /** How often the editor holds on to its lease, in milliseconds. */
  holdEvery: number;
  /** How often the viewer asks who edits the file. */
  watchEvery: number;
  /** How long the editor holds on to its lease with no input. */
  idleAfter: number;
  /** How long before it lets the lease go the editor says it will. */
  warnBefore: number;
}

/** How long an editor with no input holds on to the file's lease: 30 minutes. */
export const IDLE_AFTER = 30 * 60_000;
/** How long before then it says so: 5 minutes, time to come back to it. */
export const WARN_BEFORE = 5 * 60_000;

export const LEASE_TIMES: LeaseTimes = {
  holdEvery: 30_000,
  watchEvery: 15_000,
  idleAfter: IDLE_AFTER,
  warnBefore: WARN_BEFORE,
};

/** A request not answered in its time: it may have reached the server all the same. */
class Unanswered extends Error {}

/**
 * `promise`, or `Unanswered` once `ms` passed: a request to the server that is
 * not answered within a turn (a connection half dead, a lid closed mid-way)
 * is asked again at the next, and keeps no later turn waiting.
 */
function within<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      reject(new Unanswered('not answered within a turn'));
    }, ms);
  });
  return Promise.race([promise, late]).finally(() => {
    clearTimeout(timer);
  });
}

/** What taking a lease came to: it is held, another holds the file's, or it could not be asked. */
export type Taking = 'held' | 'editing' | 'failed';

/** The lease of a file open in the editor. */
export class EditLease {
  /** Somebody else holds the file's lease now: this one ended unheld, and was taken meanwhile. */
  lost = $state(false);
  /** Nothing was put in for `idleAfter`: the lease was let go of, and the next input takes it. */
  resting = $state(false);
  /** Nothing was put in for so long that the lease is let go of within `warnBefore`. */
  idleSoon = $state(false);
  /** While `idleSoon`: in how many minutes at most, as of the last turn (rounded up). */
  minutesLeft = $state(0);

  readonly #source: LeaseSource;
  readonly #times: LeaseTimes;
  #leaseId = '';
  #timer: ReturnType<typeof setInterval> | undefined;
  #ended = false;
  /** When the writer last put something in, by the clock. */
  #inputAt = Date.now();
  /** A request to hold on is under way: another waits for its turn. */
  #asking = false;
  /** The lease let go of for want of input, until the server answered (or a turn passed): it is taken after. */
  #resting: Promise<void> = Promise.resolve();
  /** How long a lease lasts not held on to, as the server last said, in milliseconds. */
  #term = 0;
  /**
   * Until when (by the clock) the editor may hold a lease it does not know the name of: one a
   * request given up on may have given it.
   */
  #unknownUntil = 0;

  constructor(source: LeaseSource, times: LeaseTimes = LEASE_TIMES) {
    this.#source = source;
    this.#times = times;
  }

  /** Takes the lease, and holds on to it from then until it is let go. */
  async take(): Promise<Taking> {
    let held: HeldFileLease | 'editing';
    try {
      held = await this.#source.holdLease('');
    } catch {
      return 'failed';
    }
    if (held === 'editing') return 'editing';
    this.#have(held);
    this.#inputAt = Date.now();
    if (this.#ended) {
      // Let go of before it was given.
      void this.#letGo(false);
      return 'failed';
    }
    this.#timer = setInterval(() => {
      void this.#holdOn();
    }, this.#times.holdEvery);
    document.addEventListener('visibilitychange', this.#onVisibility);
    window.addEventListener('pagehide', this.#onHide);
    window.addEventListener('pageshow', this.#onShow);
    return 'held';
  }

  /** The writer put something in (a key, the pointer, a change): a lease let go of is taken. */
  touch(): void {
    if (this.#ended) return;
    this.#inputAt = Date.now();
    this.idleSoon = false;
    if (!this.resting) return;
    this.resting = false;
    void this.#holdOn();
  }

  async #holdOn(): Promise<void> {
    if (this.#ended || this.resting || this.#asking) return;
    const idle = Date.now() - this.#inputAt;
    if (idle >= this.#times.idleAfter) {
      this.#rest();
      return;
    }
    this.idleSoon = idle >= this.#times.idleAfter - this.#times.warnBefore;
    this.minutesLeft = this.idleSoon ? Math.ceil((this.#times.idleAfter - idle) / 60_000) : 0;
    this.#asking = true;
    try {
      // Taken again only once it went (or a turn passed): asked before, it would be found still held.
      await this.#resting;
      const held = await this.#ask();
      if (held !== 'editing') this.#have(held);
      else if (!(await this.#perhapsMine())) this.lost = true;
    } catch {
      // Not reached: asked again at the next turn, within the lease's term.
      return;
    } finally {
      this.#asking = false;
    }
    // Let go of while it was asked for: what was given since goes too.
    if (this.#over()) void this.#letGo(false);
  }

  /** A lease held, as the server gave it: by its name, which may be new, and its term. */
  #have(held: HeldFileLease): void {
    // A lease that ended and that nobody took is taken anew, under another name.
    this.#leaseId = held.leaseId;
    this.#term = held.seconds * 1000;
    this.lost = false;
    // One lease of a file at a time: none other is the editor's.
    this.#unknownUntil = 0;
  }

  /** Holds on to the lease by its name, or takes the file's; waits a turn at most. */
  async #ask(): Promise<HeldFileLease | 'editing'> {
    const asked = this.#leaseId;
    const answer = this.#source.holdLease(asked);
    try {
      return await within(answer, this.#times.holdEvery);
    } catch (error) {
      if (error instanceof Unanswered) {
        this.#unknownUntil = Date.now() + this.#term;
        void answer.then(
          (late) => {
            this.#answeredLate(late, asked);
          },
          () => undefined,
        );
      }
      throw error;
    }
  }

  /**
   * A request given up on was answered after all: the lease it gave is the editor's, by its
   * name, unless the editor let go of it meanwhile (closed, or resting), or learned another's.
   */
  #answeredLate(late: HeldFileLease | 'editing', asked: string): void {
    if (late === 'editing') return;
    if (this.#ended || this.resting) {
      void this.#source.releaseLease(late.leaseId, false).catch(() => undefined);
    } else if (this.#leaseId === asked) {
      // Held on to by that name at the next turn, which tells whether it is lost.
      this.#leaseId = late.leaseId;
      this.#term = late.seconds * 1000;
    }
  }

  /**
   * Whether the file's lease, found held, may be one a request given up on gave the editor: for
   * a term after it, unless it is somebody else's (asked, a turn at most). The reader's own other
   * window is not told apart from it until then.
   */
  async #perhapsMine(): Promise<boolean> {
    if (Date.now() >= this.#unknownUntil) return false;
    const lease = await within(this.#source.lease(), this.#times.holdEvery).catch(() => undefined);
    return !(lease?.editing === true && lease.editorId !== this.#source.me);
  }

  /**
   * No input for `idleAfter`: the lease goes, and nobody else is told the writer is at work.
   * A lease another took meanwhile stays told (`lost`). Where the server was not reached the
   * lease may be held still: the next input holds on to it by its name, as at any turn.
   */
  #rest(): void {
    this.resting = true;
    this.idleSoon = false;
    const leaseId = this.#leaseId;
    this.#leaseId = '';
    if (!leaseId) return;
    // Not answered within a turn, it may not have reached the server either: held still, it is
    // held on to by its name at the next input.
    const letGo = within(this.#source.releaseLease(leaseId, false), this.#times.holdEvery);
    this.#resting = letGo.catch(async () => {
      // Closed meanwhile: let go of once more, as nothing will hold on to it.
      if (this.#ended) await this.#source.releaseLease(leaseId, false).catch(() => undefined);
      else if (!this.#leaseId) this.#leaseId = leaseId;
    });
  }

  /** Whether the lease was let go of, read anew after a wait. */
  #over(): boolean {
    return this.#ended;
  }

  readonly #onVisibility = (): void => {
    if (document.visibilityState === 'visible') void this.#holdOn();
  };

  /** The page goes (closed, or left): the lease goes with it. */
  readonly #onHide = (): void => {
    void this.#letGo(true);
  };

  /** The page came back from the browser's cache of pages left: the lease is taken anew. */
  readonly #onShow = (event: PageTransitionEvent): void => {
    if (event.persisted) void this.#holdOn();
  };

  /** Lets the lease held go; settles when the server answered, or did not. */
  #letGo(keepalive: boolean): Promise<void> {
    const leaseId = this.#leaseId;
    if (!leaseId) return Promise.resolve();
    this.#leaseId = '';
    return this.#source.releaseLease(leaseId, keepalive).catch(() => undefined);
  }

  /**
   * Lets the lease go, and stops holding on to it: the editor is closed.
   * Settles when the server answered, or did not.
   */
  release(): Promise<void> {
    if (this.#ended) return Promise.resolve();
    this.#ended = true;
    clearInterval(this.#timer);
    document.removeEventListener('visibilitychange', this.#onVisibility);
    window.removeEventListener('pagehide', this.#onHide);
    window.removeEventListener('pageshow', this.#onShow);
    return this.#letGo(false);
  }
}

/**
 * What "Edit" came to: the lease is held and the version on screen is the
 * newest (`held`), another edits the file, the lease could not be asked for,
 * or a newer version was written since the one on screen was read.
 */
export type Start = { as: 'held'; lease: EditLease } | { as: 'editing' | 'failed' | 'newer' };

/**
 * Takes a file's lease to edit its version `version`, then asks which version
 * is the newest (`newest`, undefined where unknown): a version written since
 * the one on screen was read is not edited over, and the lease goes again.
 */
export async function startEditing(
  source: LeaseSource,
  version: number,
  newest: () => Promise<number | undefined>,
): Promise<Start> {
  const lease = new EditLease(source);
  const taken = await lease.take();
  if (taken !== 'held') return { as: taken };
  const current = await newest().catch(() => undefined);
  if (current === undefined || current === version) return { as: 'held', lease };
  void lease.release();
  return { as: 'newer' };
}

/** Who edits a file, as the viewer asks it. */
export class LeaseWatch {
  /** As last read; undefined until it is, and when it could not be. */
  lease = $state.raw<FileLease>();

  readonly #source: LeaseSource;
  readonly #every: number;

  constructor(source: LeaseSource, every = LEASE_TIMES.watchEvery) {
    this.#source = source;
    this.#every = every;
  }

  /** Somebody edits the file in another window or tab: the reader in another, or somebody else. */
  get editing(): boolean {
    return this.lease?.editing === true;
  }

  /** Who: the reader (in another window), somebody named, or somebody else. */
  get editor(): { who: 'me' } | { who: 'named'; userId: string } | { who: 'someone' } | undefined {
    const lease = this.lease;
    if (!lease?.editing) return undefined;
    if (lease.editorId === '') return { who: 'someone' };
    if (lease.editorId === this.#source.me) return { who: 'me' };
    return { who: 'named', userId: lease.editorId };
  }

  /** Asks again, and learns the name of who is named. */
  async read(): Promise<void> {
    try {
      const lease = await this.#source.lease();
      const { editorId } = lease;
      if (editorId !== '' && editorId !== this.#source.me) {
        await this.#source.learnUsers([editorId]).catch(() => undefined);
      }
      this.lease = lease;
    } catch {
      this.lease = undefined;
    }
  }

  /**
   * Asks once `first` settled (a lease of the reader's being let go), and every while
   * after while the page is shown, until the returned function is called.
   */
  watch(first: Promise<unknown> = Promise.resolve()): () => void {
    void first.finally(() => this.read());
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible') void this.read();
    }, this.#every);
    return () => {
      clearInterval(timer);
    };
  }
}
