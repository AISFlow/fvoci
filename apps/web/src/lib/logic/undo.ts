/**
 * Undo and redo of the changes a person makes.
 *
 * A change is a pair of functions: `redo` performs it and `undo` takes it
 * back. Whatever `redo` returns is handed to `undo`, so a change that creates
 * something (and learns its identifier only from the server's answer) can
 * remove exactly that.
 *
 * A change is a step of the history the moment it is made, not when the
 * backend has answered: undo always takes back what the person did last.
 * Undo and redo are asked for at once (what can be undone next moves at
 * once) and carried out one after the other, each when the writes of its
 * step have come back. A change the backend refuses never happened: it
 * leaves the history, and an undo or redo that was waiting for it is
 * dropped without touching the step before it.
 *
 * A change is made at once, also while undos or redos wait. It was made on
 * the screen as it was, which did not show them yet: they are dropped, and
 * the history goes on from what is in force. One that is being carried out
 * shows already, and stays.
 *
 * Changes that share a `groupId` are one step. A gesture whose changes are
 * made one after the other (each waiting for the one before) runs inside
 * `group`: its step is in place from its first change, and an undo asked
 * for meanwhile waits for the end of the gesture. A change of the person
 * made in the middle of a gesture comes between: the rest of the gesture is
 * a step after it.
 *
 * `redo` and `undo` must not perform changes themselves.
 */

import {
  call,
  cancel,
  deferred,
  detached,
  reaim,
  ready,
  replaceCut,
  requested,
  settled,
  targetOf,
  type Action,
  type Asked,
  type Entry,
  type Gesture,
  type Step,
} from './undo-parts.ts';

let groupCounter = 0;

export class UndoManager {
  /** Called whenever what can be undone or redone, or `busy`, has changed. */
  onStateDidChange?: () => void;

  /** The steps, in the order they were made; undone ones stay until a change cuts them. */
  #entries: Entry[] = [];
  /** Undos and redos asked for and not yet over, in the order asked. */
  #asks: Asked[] = [];
  /** How many undos and redos were asked for: the number of the last one. */
  #asked = 0;
  /** How many steps were placed: the number of the last one. */
  #placed = 0;
  /** Gestures still under way, by group. */
  readonly #gestures = new Map<string, Gesture>();
  /** The turn the next undo or redo waits for. */
  #queue: Promise<unknown> = Promise.resolve();
  /** The undo or redo being carried out: the next one begins after it, also once the queue let go. */
  #carried: Promise<unknown> = Promise.resolve();
  /** Undos and redos asked for and not yet over. */
  #waiting = 0;
  /** Times the history was cleared. */
  #history = 0;

  #requested(): Map<Entry, boolean> {
    return requested(this.#entries, this.#asks);
  }

  get #index(): number {
    return [...this.#requested().values()].lastIndexOf(true);
  }

  get #inForce(): number {
    return this.#entries.map((entry) => entry.inForce && entry.steps.length > 0).lastIndexOf(true);
  }

  get #redoEntry(): Entry | undefined {
    return targetOf(this.#entries, this.#requested(), 'redo');
  }

  /** When the change that would be undone next was made; 0 without one. */
  get currentCheckpoint(): number {
    return this.#entries[this.#index]?.checkpoint ?? 0;
  }

  get undoDescription(): string | undefined {
    return this.#entries[this.#index]?.steps.at(-1)?.description;
  }

  get redoDescription(): string | undefined {
    return this.#redoEntry?.steps[0]?.description;
  }

  get canUndo(): boolean {
    return this.#index !== -1;
  }

  get canRedo(): boolean {
    return this.#redoEntry !== undefined;
  }

  /** Whether an undo or redo is waiting for a write, or being carried out. */
  get busy(): boolean {
    return this.#waiting > 0;
  }

  /**
   * Performs a change and remembers how to take it back: the step is in place
   * at once. Returns what `redo` returned; rejects, and forgets the change,
   * when `redo` rejects. A discardable change takes the checkpoint of the
   * change before it instead of the current time (0 when it is the first).
   */
  async perform<T>(
    redo: () => Promise<T>,
    undo: (value: T) => Promise<void>,
    description?: string,
    groupId?: string,
    isDiscardable = false,
    ready?: () => Promise<void> | undefined,
  ): Promise<T> {
    const step: Step = {
      undo: undo as (value?: unknown) => Promise<void>,
      redo,
      description,
      value: undefined,
      ready,
    };
    const entry = this.#place(step, groupId, isDiscardable);
    const making = call(redo);
    entry.making.add(making);
    try {
      const value = await making;
      step.value = value;
      return value;
    } catch (error) {
      entry.steps = entry.steps.filter((other) => other !== step);
      this.#forgetIfEmpty(entry);
      throw error;
    } finally {
      entry.making.delete(making);
    }
  }

  /**
   * Runs a gesture whose changes (performed with the `groupId` it is given)
   * are one step, in place from the first of them; with `placedAtOnce`, from
   * its start (a gesture that reads before it changes anything: an undo
   * asked for during the reading is of it, and it is of the history it was
   * begun in). An undo asked for while the gesture is under way waits for
   * its end. A gesture that changes nothing leaves the history as it was:
   * a step placed at once is no change until its first one, so the undos
   * and redos asked for before it are dropped then, not when it is placed.
   */
  async group<T>(gesture: (groupId: string) => Promise<T>, placedAtOnce = false): Promise<T> {
    groupCounter += 1;
    const groupId = `group-${groupCounter}`;
    const over = deferred();
    const record: Gesture = {
      over: over.promise,
      entry: undefined,
      entries: [],
      history: this.#history,
    };
    this.#gestures.set(groupId, record);
    if (placedAtOnce) {
      this.#begin(record, groupId, false, true);
      this.onStateDidChange?.();
    }
    try {
      return await gesture(groupId);
    } finally {
      this.#gestures.delete(groupId);
      over.resolve();
      for (const entry of record.entries) {
        entry.gesture = undefined;
        this.#forgetIfEmpty(entry);
      }
    }
  }

  /**
   * Takes back the last step that is not undone. What can be undone next
   * moves at once; the step is taken back when its writes have come back.
   * Rejects when taking it back fails (it then stays the one to undo).
   */
  async undo(): Promise<this> {
    const entry = this.#entries[this.#index];
    if (!entry) return this;
    await this.#ask(entry, 'undo');
    return this;
  }

  /** Does again the step undone last; as `undo`, the other way. */
  async redo(): Promise<this> {
    const entry = this.#redoEntry;
    if (!entry) return this;
    await this.#ask(entry, 'redo');
    return this;
  }

  /**
   * Forgets every step. Undos and redos still waiting are dropped; changes,
   * undos and redos still under way end without touching the new history
   * (their steps are no longer in it), and a gesture begun before is not
   * remembered.
   */
  clear(): void {
    this.#history += 1;
    // What is still carried out is of the history left: the new one waits for none of it.
    this.#carried = Promise.resolve();
    this.#waiting = 0;
    const hadEntries = this.#entries.length > 0;
    this.#entries = [];
    this.#drop();
    if (hadEntries) this.onStateDidChange?.();
  }

  /** The step `step` belongs to: its gesture's, the last step's of its group, or a new one. */
  #place(step: Step, groupId: string | undefined, isDiscardable: boolean): Entry {
    const gesture = groupId === undefined ? undefined : this.#gestures.get(groupId);
    const last = this.#entries[this.#index];
    let entry = gesture
      ? this.#continued(gesture)
      : groupId !== undefined && last?.groupId === groupId
        ? last
        : undefined;
    entry ??= gesture
      ? this.#begin(gesture, groupId, isDiscardable)
      : this.#push(groupId, isDiscardable);
    this.#commitReservation(entry);
    entry.steps.push(step);
    this.onStateDidChange?.();
    return entry;
  }

  /** A new step of `gesture`. */
  #begin(
    gesture: Gesture,
    groupId: string | undefined,
    isDiscardable: boolean,
    reserve = false,
  ): Entry {
    // A gesture begun in a history since cleared is of that history: its step is in none.
    const entry =
      gesture.history !== this.#history
        ? detached(groupId)
        : this.#push(groupId, isDiscardable, reserve);
    gesture.entry = entry;
    gesture.entries.push(entry);
    entry.gesture = gesture;
    return entry;
  }

  /**
   * The step a gesture's next change joins: its own, unless a change of the
   * person was made after it (a step of another gesture that has changed
   * nothing yet is none). That change came between: the rest of the gesture
   * is a new change after it, so that undo and redo go in the order the
   * changes were made, and it drops the undos and redos waiting as a new
   * change does (asked of the steps in another order, carried out over it
   * they would write values that never were).
   */
  #continued(gesture: Gesture): Entry | undefined {
    const { entry } = gesture;
    const at = entry ? this.#entries.indexOf(entry) : -1;
    if (at === -1) return entry;
    const between = this.#entries.slice(at + 1);
    return between.some((other) => other.steps.length > 0 || !other.gesture) ? undefined : entry;
  }

  /**
   * A new step after the last one in force. Undos and redos that wait are
   * dropped; what could be redone is gone (kept in `cut`). A reservation
   * (`reserve`) is placed last and drops and cuts nothing until its first
   * change (`#commitReservation`); meanwhile what could be redone is hidden.
   */
  #push(groupId: string | undefined, isDiscardable: boolean, reserve = false): Entry {
    this.#placed += 1;
    // A reservation can still end without a change: it must not discard requests or redo yet.
    if (!reserve) this.#drop();
    const requested = this.#requested();
    const hidden = reserve ? this.#entries.filter((entry) => !requested.get(entry)) : [];
    const cut = reserve ? [] : this.#entries.filter((entry) => !entry.inForce);
    if (!reserve) this.#entries = this.#entries.filter((entry) => entry.inForce);
    const checkpoint = isDiscardable ? (this.#entries.at(-1)?.checkpoint ?? 0) : Date.now();
    const entry: Entry = {
      ...detached(groupId),
      checkpoint,
      cut,
      hidden,
      reservedAt: reserve ? this.#asked : undefined,
      placed: this.#placed,
    };
    this.#entries.push(entry);
    return entry;
  }

  /**
   * The first change of a step placed at once: the undos and redos asked for
   * before it are dropped and what could be redone then is cut, as by any
   * change; those asked for since (of this gesture) stay.
   */
  #commitReservation(entry: Entry): void {
    const { reservedAt } = entry;
    if (reservedAt === undefined) return;
    entry.hidden = [];
    entry.reservedAt = undefined;
    const at = this.#entries.indexOf(entry);
    // Of a history since cleared: the history in place is none of its business.
    if (at === -1) return;
    this.#drop(reservedAt);
    const cut = this.#entries.slice(0, at).filter((other) => !other.inForce);
    this.#entries = this.#entries.filter((other) => !cut.includes(other));
    entry.cut.push(...cut);
    reaim(this.#entries, this.#asks, reservedAt);
  }

  /**
   * A step left without changes (all refused, or a gesture that changed
   * nothing) leaves the history. When it is still the last step, what could
   * be redone before it was made comes back.
   */
  #forgetIfEmpty(entry: Entry): void {
    if (entry.steps.length > 0 || entry.gesture) return;
    const at = this.#entries.indexOf(entry);
    if (at === -1) return;
    this.#entries.splice(at, 1);
    // What it cut comes back once nothing made after it stays: the step after it keeps it until then.
    const next = this.#entries[at];
    if (!next) this.#entries.push(...entry.cut.filter((other) => other.steps.length > 0));
    else if (next.cut.length === 0) next.cut = entry.cut;
    this.onStateDidChange?.();
  }

  /** Drops the undos and redos asked for and not yet begun. */
  #drop(through = Infinity): void {
    for (const ask of this.#asks) {
      if (ask.number <= through) cancel(ask);
    }
  }

  /** Queues an undo or redo of `entry`, and waits for it to be over. */
  async #ask(aimedAt: Entry, action: Action): Promise<void> {
    this.#asked += 1;
    const ask: Asked = {
      number: this.#asked,
      entry: aimedAt,
      action,
      cancelled: false,
      begun: false,
      seen: this.#placed,
      dropped: deferred(),
    };
    this.#asks.push(ask);
    const current = (): boolean => !ask.cancelled;
    const history = this.#history;
    this.#waiting += 1;
    this.onStateDidChange?.();
    const turn = this.#queue.then(async () => {
      // Waits for the writes of its step, and of the one it is aimed at again meanwhile.
      let target: Entry | undefined;
      while (current() && target !== ask.entry) {
        target = ask.entry;
        await Promise.race([settled(target), ask.dropped.promise]);
        if (!current()) return;
        // An undo waits, still dropped by a new change, for the writes its inverses take back to.
        if (action === 'undo') await Promise.race([ready(target), ask.dropped.promise]);
        if (!current()) return;
        // A dropped queue can move on, but a write already being carried out still comes first.
        await this.#carried;
      }
      const { entry } = ask;
      if (!current() || !this.#entries.includes(entry)) return;
      ask.begun = true;
      entry.inForce = action === 'redo';
      const carrying = this.#carryOut(entry, action);
      entry.carrying = carrying.catch(() => undefined);
      this.#carried = entry.carrying;
      try {
        await carrying;
      } finally {
        entry.carrying = undefined;
      }
    });
    this.#queue = Promise.race([turn.catch(() => undefined), ask.dropped.promise]);
    try {
      await turn;
    } finally {
      this.#asks = this.#asks.filter((other) => other !== ask);
      if (history === this.#history) this.#waiting -= 1;
      this.onStateDidChange?.();
    }
  }

  async #carryOut(entry: Entry, action: Action): Promise<void> {
    const { steps } = entry;
    if (action === 'undo') {
      for (const [at, step] of [...steps.entries()].reverse()) {
        try {
          await step.undo(step.value);
        } catch (error) {
          this.#failed(entry, at + 1);
          throw error;
        }
      }
      return;
    }
    for (const [at, step] of steps.entries()) {
      try {
        step.value = await step.redo();
      } catch (error) {
        this.#failed(entry, at);
        throw error;
      }
    }
  }

  /**
   * An undo or redo of `entry` failed with its first `done` changes in force
   * and the rest not. The step is split there and the history made to say
   * what is in force; the undos and redos asked for after it are dropped,
   * unless the step was undone and a newer change has taken its place.
   */
  #failed(entry: Entry, done: number): void {
    const piece = (steps: Step[], inForce: boolean): Entry => ({
      ...entry,
      steps,
      inForce,
      reservedAt: undefined,
      hidden: [],
      making: new Set(),
      carrying: undefined,
      gesture: undefined,
      cut: [],
    });
    const inForce = piece(entry.steps.slice(0, done), true);
    const notInForce = piece(entry.steps.slice(done), false);
    const at = this.#entries.indexOf(entry);
    if (at === -1) {
      // A change made while it was undone took its place (perhaps in what it cut in
      // turn): what is still in force goes before the step of the history that holds it.
      if (inForce.steps.length === 0) return;
      const rest = notInForce.steps.length > 0 ? notInForce : undefined;
      const host = this.#entries.findIndex((other) => replaceCut(other, entry, rest));
      if (host === -1) return;
      const asked = this.#index;
      this.#entries.splice(host, 0, inForce);
      // Undos asked for down to the newer change still hold; one asked for below it did not
      // know that this step stays.
      if (asked >= host - 1) {
        this.onStateDidChange?.();
        return;
      }
    } else if (this.#inForce > at) {
      // A change made while it was redone comes after it: what is not in force is gone.
      this.#entries.splice(at, 1, ...(inForce.steps.length > 0 ? [inForce] : []));
    } else {
      const pieces = [inForce, notInForce].filter((piece) => piece.steps.length > 0);
      this.#entries.splice(at, 1, ...pieces);
    }
    this.#drop();
    this.onStateDidChange?.();
  }
}
