/**
 * Which pages of a wiki's tree are open (their children shown), for the
 * reader on this browser: kept per person and wiki in the browser's storage,
 * and for the visit alone where there is none (blocked, a private window).
 * A page that is not open is folded: a new wiki's tree shows its top.
 */
import { SvelteSet } from 'svelte/reactivity';

const KEY = 'fvoci:wiki-open:';

export class TreeOpen {
  /** None while the reader is not known yet: the tree is then kept for the visit alone. */
  readonly #key: string | undefined;
  readonly #open: SvelteSet<string>;

  constructor(userId: string, containerId: string, prefix = KEY) {
    this.#key = userId === '' ? undefined : `${prefix}${userId}:${containerId}`;
    this.#open = new SvelteSet(this.#read());
  }

  #read(): string[] {
    if (this.#key === undefined) return [];
    try {
      const kept: unknown = JSON.parse(localStorage.getItem(this.#key) ?? '[]');
      return Array.isArray(kept) ? kept.filter((id) => typeof id === 'string') : [];
    } catch {
      return [];
    }
  }

  #keep(): void {
    if (this.#key === undefined) return;
    try {
      localStorage.setItem(this.#key, JSON.stringify([...this.#open]));
    } catch {
      // Without storage the tree stays as it is for the visit.
    }
  }

  has(pageId: string): boolean {
    return this.#open.has(pageId);
  }

  /** Opens the pages, or folds them (`open` false). */
  set(pageIds: readonly string[], open = true): void {
    const changing = pageIds.filter((id) => this.#open.has(id) !== open);
    if (changing.length === 0) return;
    for (const id of changing) {
      if (open) this.#open.add(id);
      else this.#open.delete(id);
    }
    this.#keep();
  }
}
