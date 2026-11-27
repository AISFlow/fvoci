/** Records and promise helpers shared by the undo history. */
export interface Step {
  undo: (value?: unknown) => Promise<void>;
  redo: () => Promise<unknown>;
  description: string | undefined;
  value: unknown;
  /** What its undo waits for before it begins (writes its inverse takes back to); none when undefined. */
  ready: (() => Promise<void> | undefined) | undefined;
}

export interface Entry {
  /** What has actually begun, independently of queued requests. */
  inForce: boolean;
  /** Of a reservation: the number of the last ask before it; undefined once it changes something. */
  reservedAt: number | undefined;
  /** Redo entries hidden provisionally while a reservation may still change nothing. */
  hidden: Entry[];
  /** The how-manieth step placed it is (0 in no history). */
  placed: number;
  checkpoint: number;
  groupId: string | undefined;
  /** In the order they were made. */
  steps: Step[];
  /** The writes of changes of this step that have not come back. */
  making: Set<Promise<unknown>>;
  /** The undo or redo of this step being carried out. */
  carrying: Promise<unknown> | undefined;
  /** The gesture of a `group`, while it is under way. */
  gesture: Gesture | undefined;
  /** What could be redone when this step was made: back if it ends with no change. */
  cut: Entry[];
}

/** A gesture under way, and its step once it has made a change. */
export interface Gesture {
  over: Promise<void>;
  /** The step its next change joins. */
  entry: Entry | undefined;
  /** Every step it made: more than one where a change of the person came between its own. */
  entries: Entry[];
  /** The history it was begun in (`clear` begins another). */
  history: number;
}

export type Action = 'undo' | 'redo';

/** An undo or redo asked for, until it is over. */
export interface Asked {
  /** The how-manieth ask it is. */
  number: number;
  /** The step it takes back or does again: aimed again while it has not begun. */
  entry: Entry;
  action: Action;
  /** Dropped: it does nothing. */
  cancelled: boolean;
  /** Carried out (or being), on its step as it was then. */
  begun: boolean;
  /** The number of the last step placed when it was asked for: it is of none placed after. */
  seen: number;
  dropped: ReturnType<typeof deferred>;
}

/** Whether each step is to be in force: what has begun, then `asks` not dropped, in order. */
export function requested(entries: readonly Entry[], asks: readonly Asked[]): Map<Entry, boolean> {
  const wanted = new Map(entries.map((entry) => [entry, entry.inForce]));
  for (const ask of asks) {
    if (!ask.cancelled && wanted.has(ask.entry)) wanted.set(ask.entry, ask.action === 'redo');
  }
  return wanted;
}

/** The step an undo or redo asked for over `wanted` (of `entries`, in order) is of. */
export function targetOf(
  entries: readonly Entry[],
  wanted: Map<Entry, boolean>,
  action: Action,
): Entry | undefined {
  const index = [...wanted.values()].lastIndexOf(true);
  if (action === 'undo') return entries[index];
  // What a reservation hid is not to be redone while it may still change something.
  const hidden = new Set(
    entries.flatMap((entry) => (entry.reservedAt === undefined ? [] : entry.hidden)),
  );
  return entries.slice(index + 1).find((entry) => !hidden.has(entry));
}

/**
 * Aims again the undos and redos asked for after the ask numbered `after`
 * and not begun: each was asked of the history as the asks before it were
 * to leave it, and some of those were dropped. Each is aimed among the steps
 * placed before it was asked for; one left with nothing to take is dropped.
 */
export function reaim(entries: readonly Entry[], asks: readonly Asked[], after: number): void {
  const wanted = requested(
    entries,
    asks.filter((ask) => ask.number <= after || ask.begun),
  );
  for (const ask of asks) {
    if (ask.cancelled || ask.begun || ask.number <= after) continue;
    const seen = entries.filter((entry) => entry.placed <= ask.seen);
    const target = targetOf(
      seen,
      new Map(seen.map((entry) => [entry, wanted.get(entry) ?? false])),
      ask.action,
    );
    if (!target) {
      cancel(ask);
      continue;
    }
    if (target !== ask.entry) {
      ask.entry = target;
    }
    wanted.set(target, ask.action === 'redo');
  }
}

/** Drops `ask`. */
export function cancel(ask: Asked): void {
  if (ask.cancelled) return;
  ask.cancelled = true;
  ask.dropped.resolve();
}

/** A step in no history. */
export function detached(groupId: string | undefined): Entry {
  return {
    inForce: true,
    reservedAt: undefined,
    hidden: [],
    placed: 0,
    checkpoint: 0,
    groupId,
    steps: [],
    making: new Set(),
    carrying: undefined,
    gesture: undefined,
    cut: [],
  };
}

/**
 * Puts `by` in place of `entry` in what `holder` cut, however deep (takes it
 * out without `by`: nothing of it can be redone); whether it was there.
 */
export function replaceCut(holder: Entry, entry: Entry, by: Entry | undefined): boolean {
  const at = holder.cut.indexOf(entry);
  if (at !== -1) {
    if (by) holder.cut[at] = by;
    else holder.cut.splice(at, 1);
    return true;
  }
  return holder.cut.some((other) => replaceCut(other, entry, by));
}

/** Waits for what the undos of the steps of `entry` wait for before they begin. */
export async function ready(entry: Entry): Promise<void> {
  await Promise.all(entry.steps.flatMap((step) => step.ready?.() ?? []));
}

/** Calls `run` now, turning a throw into a rejection. */
export async function call<T>(run: () => Promise<T>): Promise<T> {
  return run();
}

export function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve = (): void => undefined;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/** Waits until the gesture of `entry` is over and its writes, and an undo or redo of it, are done. */
export async function settled(entry: Entry): Promise<void> {
  while (entry.gesture || entry.making.size > 0 || entry.carrying) {
    await Promise.allSettled([entry.gesture?.over, entry.carrying, ...entry.making]);
  }
}
