/**
 * What a gallery shows of its cards' bodies: read when a gallery is first
 * shown (`Backend.cardPreviews`), reused across view tab switches on the same
 * open `BoardStore` (§1.3.6), and kept current while the board is active by
 * change messages, whose cards carry `preview`.
 */
import { SvelteMap } from 'svelte/reactivity';

import type { CardPreview } from '../editor/card-preview.ts';

import type { Backend } from './backend.ts';
import { isCardBlock } from './backend.ts';

export class CardPreviews {
  readonly #backend: Backend;
  readonly #boardId: string;
  readonly #previews = new SvelteMap<string, CardPreview>();
  /** Cards told of since the reading began: consulted during read, never read by UI (§1.2.5). */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- non-reactive bookkeeping set
  readonly #told = new Set<string>();
  #reading = 0;
  #stopWatching: (() => void) | undefined;
  #loaded = false;

  constructor(backend: Backend, boardId: string) {
    this.#backend = backend;
    this.#boardId = boardId;
  }

  /** The preview of a card, once read; none while it is not. */
  of(cardId: string): CardPreview | undefined {
    return this.#previews.get(cardId);
  }

  /**
   * Starts reading and watching once for the lifetime of an open board visit
   * (`BoardStore.previews`), reusing already-fetched previews across view tab switches.
   */
  retain(): () => void {
    if (!this.#stopWatching) {
      this.#stopWatching = this.#watch();
    }
    if (!this.#loaded) {
      this.#loaded = true;
      void this.#read();
    }
    return () => undefined;
  }

  /** Detaches live watching when the owning `BoardStore` is parked or closed. */
  dispose(): void {
    this.#stopWatching?.();
    this.#stopWatching = undefined;
    this.#loaded = false;
    this.#reading += 1;
  }

  /** Standalone start/stop lifecycle (used directly by unit tests). */
  start(): () => void {
    const stop = this.#watch();
    void this.#read();
    return () => {
      stop();
      this.#reading += 1;
    };
  }

  #watch(): () => void {
    return this.#backend.watchContainer(this.#boardId, (change) => {
      if (change.kind === 'blocks') {
        for (const block of change.blocks) {
          if (!isCardBlock(block) || !block.preview) continue;
          this.#told.add(block.id);
          this.#previews.set(block.id, block.preview);
        }
      } else if (change.kind === 'resync') {
        void this.#read();
      }
    });
  }

  async #read(): Promise<void> {
    this.#reading += 1;
    const reading = this.#reading;
    this.#told.clear();
    try {
      const previews = await this.#backend.cardPreviews(this.#boardId);
      if (reading !== this.#reading) return;
      for (const [cardId, preview] of previews) {
        if (!this.#told.has(cardId)) this.#previews.set(cardId, preview);
      }
    } catch {
      this.#loaded = false;
    }
  }
}
