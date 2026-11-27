/**
 * Inverse patches follow the writes their values came from. A refusal only
 * corrects dependent values: an equal value written successfully later has
 * its own origin. Definitions in a board's list are tracked by identifier;
 * a definition remains one value (its type and options belong together).
 *
 * An inverse that takes back to the value of a write still under way waits
 * for that write before it is shown and sent (`ready`): it is then sent
 * corrected when that write is refused, never with the value the server
 * refused.
 */
import { InverseValues, sameValue } from './inverse-values.ts';

type Patch = Record<string, unknown>;

interface Origin {
  value: unknown;
  beforeValue: unknown;
  before: Origin | undefined;
  state: 'pending' | 'stored' | 'refused';
  /** Settles when its write has answered. */
  over: Promise<void>;
  end: () => void;
}

interface Kept {
  patch: Patch;
  origins: Map<string, Origin>;
}

/** The nearest value that was not refused, and its origin if it can still fail. */
function surviving(origin: Origin): { value: unknown; origin?: Origin } {
  let current = origin;
  while (current.state === 'refused') {
    if (!current.before) return { value: current.beforeValue };
    current = current.before;
  }
  return { value: current.value, origin: current.state === 'pending' ? current : undefined };
}

export class Inverses<P extends object> {
  readonly #values: InverseValues;
  readonly #shown = new Map<string, Map<string, Origin>>();
  readonly #kept = new Set<Kept>();

  constructor(updated: string, deleted: string) {
    this.#values = new InverseValues(updated, deleted);
  }

  /**
   * What to wait for until no value of `patch` (an inverse kept) comes from a
   * write under way; undefined when nothing is (the change is shown at once).
   */
  ready(patch: P): Promise<void> | undefined {
    const under = this.#under(patch);
    if (under.length === 0) return undefined;
    return (async () => {
      // A refusal may hand the value on to an earlier write still under way.
      for (let left = under; left.length > 0; left = this.#under(patch)) {
        await Promise.all(left.map((origin) => origin.over));
      }
    })();
  }

  #under(patch: P): Origin[] {
    const kept = [...this.#kept].find((one) => one.patch === patch);
    return [...(kept?.origins.values() ?? [])].filter((origin) => origin.state === 'pending');
  }

  /** Keeps only the values whose currently shown source can still fail. */
  keep(id: string, patch: P): void {
    const shown = this.#shown.get(id);
    if (!shown) return;
    const origins = new Map<string, Origin>();
    for (const [key, value] of this.#values.read(patch as Patch)) {
      const origin = shown.get(key);
      if (origin && sameValue(origin.value, value)) origins.set(key, origin);
    }
    if (origins.size > 0) this.#kept.add({ patch: patch as Patch, origins });
  }

  /**
   * Sends a write; a refusal corrects only the inverse values descended from
   * that write. `send` is told which keys the screen still shows as this
   * write left them.
   */
  async write<T>(
    id: string,
    patch: P,
    reverse: P,
    send: (own: (key: string) => boolean) => Promise<T>,
    refused?: (restore: P) => void,
  ): Promise<T> {
    const origins = this.#begin(id, patch, reverse);
    try {
      // Whether the screen's value of a key is still this write's (no later change replaced it).
      const answer = await send(
        (key) => origins.has(key) && this.#shown.get(id)?.get(key) === origins.get(key),
      );
      for (const origin of origins.values()) {
        origin.state = 'stored';
        // A successful value does not depend on what came before it any more.
        origin.before = undefined;
      }
      return answer;
    } catch (error) {
      for (const origin of origins.values()) origin.state = 'refused';
      this.#correct();
      const restore: Patch = {};
      for (const [key, origin] of origins) {
        if (this.#shown.get(id)?.get(key) === origin) {
          this.#values.put(restore, key, surviving(origin).value);
        }
      }
      refused?.(restore as P);
      throw error;
    } finally {
      for (const origin of origins.values()) origin.end();
      this.#release(id);
    }
  }

  #begin(id: string, patch: P, reverse: P): Map<string, Origin> {
    const shown = this.#shown.get(id) ?? new Map<string, Origin>();
    const before = this.#values.read(reverse as Patch);
    const origins = new Map<string, Origin>();
    for (const [key, value] of this.#values.read(patch as Patch)) {
      const beforeValue = before.get(key);
      const previous = shown.get(key);
      let end = (): void => undefined;
      const over = new Promise<void>((resolve) => {
        end = resolve;
      });
      const origin: Origin = {
        value,
        beforeValue,
        before: previous && sameValue(previous.value, beforeValue) ? previous : undefined,
        state: 'pending',
        over,
        end,
      };
      origins.set(key, origin);
      shown.set(key, origin);
    }
    if (shown.size > 0) this.#shown.set(id, shown);
    return origins;
  }

  #correct(): void {
    for (const kept of this.#kept) {
      const values = this.#values.read(kept.patch);
      for (const [key, origin] of kept.origins) {
        if (origin.state !== 'refused') continue;
        const replacement = surviving(origin);
        if (sameValue(values.get(key), origin.value)) {
          this.#values.put(kept.patch, key, replacement.value);
        }
        if (replacement.origin) kept.origins.set(key, replacement.origin);
        else kept.origins.delete(key);
      }
    }
  }

  /** Unrelated writes cannot keep these references alive. */
  #release(id: string): void {
    const shown = this.#shown.get(id);
    for (const [key, origin] of shown ?? []) {
      const replacement = surviving(origin).origin;
      if (replacement) shown?.set(key, replacement);
      else shown?.delete(key);
    }
    if (shown?.size === 0) this.#shown.delete(id);
    for (const kept of this.#kept) {
      for (const [key, origin] of kept.origins) {
        if (origin.state !== 'pending') kept.origins.delete(key);
      }
      if (kept.origins.size === 0) this.#kept.delete(kept);
    }
  }
}
