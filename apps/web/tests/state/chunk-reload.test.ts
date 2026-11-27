/**
 * Stale-deployment detection (`chunk-reload.svelte.ts`): compares the live
 * `index.html` module entry script against the current document's and settles
 * active Yjs bodies before raising `staleBuild.detected`.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  noteChunkFailure,
  registerActiveBodySettler,
  staleBuild,
} from '$lib/chunk-reload.svelte.ts';

afterEach(() => {
  staleBuild.resetForTests();
  document.head.replaceChildren();
  document.body.replaceChildren();
  vi.unstubAllGlobals();
});

function setCurrentScript(src: string): void {
  const script = document.createElement('script');
  script.type = 'module';
  script.setAttribute('src', src);
  document.head.appendChild(script);
}

describe('stale deployment detection on chunk load failure', () => {
  it('stays quiet when index.html still points to the same entry script', async () => {
    setCurrentScript('/index-abc123.js');
    vi.stubGlobal('fetch', () =>
      Promise.resolve(
        new Response('<!doctype html><script type="module" src="/index-abc123.js"></script>', {
          headers: { 'Content-Type': 'text/html' },
        }),
      ),
    );
    expect(await noteChunkFailure()).toBe(false);
    expect(staleBuild.detected).toBe(false);
  });

  it('settles active collaborative bodies first and raises detected when a new build is deployed', async () => {
    setCurrentScript('/index-old111.js');
    const order: string[] = [];
    const unregister = registerActiveBodySettler(async () => {
      await Promise.resolve();
      order.push('settled');
      expect(staleBuild.detected).toBe(false);
    });
    vi.stubGlobal('fetch', () =>
      Promise.resolve(
        new Response('<!doctype html><script type="module" src="/index-new222.js"></script>', {
          headers: { 'Content-Type': 'text/html' },
        }),
      ),
    );

    expect(await noteChunkFailure()).toBe(true);
    order.push('detected');
    expect(order).toEqual(['settled', 'detected']);
    expect(staleBuild.detected).toBe(true);
    unregister();
  });

  it('probes under meta[name="fvoci:base"] when deployed on a subpath', async () => {
    const meta = document.createElement('meta');
    meta.setAttribute('name', 'fvoci:base');
    meta.setAttribute('content', '/boards/');
    document.head.appendChild(meta);
    setCurrentScript('/boards/index-old111.js');

    const requested: string[] = [];
    vi.stubGlobal('fetch', (input: string) => {
      requested.push(input);
      return Promise.resolve(
        new Response(
          '<!doctype html><script type="module" src="/boards/index-new222.js"></script>',
          { headers: { 'Content-Type': 'text/html' } },
        ),
      );
    });

    expect(await noteChunkFailure()).toBe(true);
    expect(requested).toEqual(['/boards/']);
    expect(staleBuild.detected).toBe(true);
  });
});
