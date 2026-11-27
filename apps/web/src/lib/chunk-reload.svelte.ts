/**
 * Lazy chunk loading and stale-deployment detection (`BoardPage.newVersion`).
 *
 * When a dynamic `import()` fails in an open tab after a new deployment,
 * `noteChunkFailure()` fetches `index.html` (`cache: 'no-store'`) and compares
 * its entry `<script type="module" src>` with the current document's script.
 * When a newer build is live, active collaborative Yjs rooms are settled first
 * (`settleActiveBodies()`) before exposing the 1-click reload banner.
 */

import { buildUrl, normalizeBasePath } from './logic/urls.ts';

/** The base path the server filled into index.html (`meta[name="fvoci:base"]`); empty at the site root. */
export function readBasePath(document: Document): string {
  const raw = document.querySelector('meta[name="fvoci:base"]')?.getAttribute('content') ?? '';
  // Served as a plain file (the development server), the placeholder is still there.
  return raw.startsWith('{{') ? '' : normalizeBasePath(raw);
}

// eslint-disable-next-line svelte/prefer-svelte-reactivity -- internal callback registry, not read in templates
const bodySettlers = new Set<() => Promise<void>>();

/** Registers an active collaborative body room so its buffered edits settle before reload. */
export function registerActiveBodySettler(settle: () => Promise<void>): () => void {
  bodySettlers.add(settle);
  return () => {
    bodySettlers.delete(settle);
  };
}

/** Flushes all open collaborative Yjs rooms (`DocRoom.settle()`). */
export async function settleActiveBodies(): Promise<void> {
  await Promise.allSettled([...bodySettlers].map((settle) => settle()));
}

class StaleBuildState {
  detected = $state(false);
  #checking: Promise<boolean> | undefined;

  resetForTests(): void {
    this.detected = false;
    this.#checking = undefined;
  }

  check(): Promise<boolean> {
    if (this.detected) return Promise.resolve(true);
    if (typeof document === 'undefined' || typeof fetch === 'undefined') {
      return Promise.resolve(false);
    }
    this.#checking ??= this.#probe().finally(() => {
      this.#checking = undefined;
    });
    return this.#checking;
  }

  async #probe(): Promise<boolean> {
    const currentSrc = entryScriptSrc(document);
    if (!currentSrc) return false;
    const base = readBasePath(document);
    try {
      const response = await fetch(buildUrl('/', base), {
        cache: 'no-store',
        credentials: 'same-origin',
        headers: { Accept: 'text/html' },
      });
      if (!response.ok) return false;
      const html = await response.text();
      const parsed = new DOMParser().parseFromString(html, 'text/html');
      const nextSrc = entryScriptSrc(parsed);
      if (!nextSrc || nextSrc === currentSrc) return false;
      await settleActiveBodies();
      this.detected = true;
      return true;
    } catch {
      return false;
    }
  }
}

function entryScriptSrc(root: ParentNode): string | undefined {
  const script = root.querySelector<HTMLScriptElement>('script[type="module"][src]');
  const raw = script?.getAttribute('src');
  if (!raw) return undefined;
  try {
    return new URL(raw, location.origin).pathname;
  } catch {
    return raw;
  }
}

export const staleBuild = new StaleBuildState();

/** Checks whether a failed dynamic `import()` was caused by a new deployment. */
export function noteChunkFailure(): Promise<boolean> {
  return staleBuild.check();
}

/** Settles any active collaborative bodies, then reloads into the new deployment. */
export async function reloadForNewVersion(): Promise<void> {
  await settleActiveBodies();
  location.reload();
}

/** Reactive wrapper for dynamically imported screen or view components. */
export class LazyChunk<T> {
  value = $state.raw<T | undefined>(undefined);
  failed = $state(false);
  readonly #load: () => Promise<{ default: T }>;
  #inflight: Promise<void> | undefined;

  constructor(load: () => Promise<{ default: T }>) {
    this.#load = load;
  }

  ensure(): void {
    if (this.value || this.#inflight) return;
    this.failed = false;
    this.#inflight = this.#load()
      .then((mod) => {
        this.value = mod.default;
      })
      .catch(() => {
        this.failed = true;
        void noteChunkFailure();
      })
      .finally(() => {
        this.#inflight = undefined;
      });
  }
}
