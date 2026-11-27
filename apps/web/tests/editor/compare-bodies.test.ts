/**
 * Where two bodies are compared (compare-bodies.ts): in a worker, and here
 * when the worker fails; a worker that failed is started anew for the next
 * comparison, until one has failed three times, when the page compares them
 * itself from then on.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { diffBodies } from '$lib/logic/body-diff.ts';

/** The workers started, and whether the next ones fail. */
const workers = vi.hoisted(() => ({ started: 0, failing: true }));

vi.mock('$lib/editor/diff-worker.ts?worker', () => ({
  default: class {
    readonly #listeners = new Map<string, ((event: unknown) => void)[]>();

    constructor() {
      workers.started += 1;
    }

    addEventListener(type: string, listener: (event: unknown) => void): void {
      this.#listeners.set(type, [...(this.#listeners.get(type) ?? []), listener]);
    }

    postMessage(asked: { id: number; before: never; after: never }): void {
      const failing = workers.failing;
      queueMicrotask(() => {
        for (const listener of this.#listeners.get(failing ? 'error' : 'message') ?? [])
          listener({ data: { id: asked.id, diff: diffBodies(asked.before, asked.after) } });
      });
    }

    terminate(): void {
      this.#listeners.clear();
    }
  },
}));

const said = (value: string): never =>
  ({
    type: 'doc',
    content: [{ type: 'paragraph', content: [{ type: 'text', text: value }] }],
  }) as never;

afterEach(() => {
  vi.resetModules();
});

describe('a comparison', () => {
  it('is made here when the worker fails, and in a new worker the next time', async () => {
    const { compareBodies } = await import('$lib/editor/compare-bodies.ts');
    workers.started = 0;
    workers.failing = true;
    expect((await compareBodies(said('one'), said('two'))).added).toBe(1);
    expect(workers.started).toBe(1);
    workers.failing = false;
    expect((await compareBodies(said('one'), said('two three'))).added).toBe(2);
    expect(workers.started).toBe(2);
    // The worker that answers is kept for the comparisons after.
    await compareBodies(said('one'), said('two'));
    expect(workers.started).toBe(2);
  });

  it('is made here for good once a worker has failed three times', async () => {
    const { compareBodies } = await import('$lib/editor/compare-bodies.ts');
    workers.started = 0;
    workers.failing = true;
    for (let time = 0; time < 4; time += 1)
      expect((await compareBodies(said('one'), said('two'))).changed).toBe(true);
    expect(workers.started).toBe(3);
  });
});
