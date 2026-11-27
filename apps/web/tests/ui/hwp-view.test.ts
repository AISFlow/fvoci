import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { renderer } from '../../src/viewer-frame/hwp.ts';
import { HwpView } from '../../src/viewer-frame/hwp-view.ts';
import type { Report } from '../../src/viewer-frame/renderer.ts';
import {
  WorkerClient,
  WorkerClosed,
  type WorkerLike,
} from '../../src/viewer-frame/worker-client.ts';

/** A worker that never answers unless told to, and records what it was sent. */
class FakeWorker implements WorkerLike {
  readonly #events = new EventTarget();
  sent: unknown[] = [];
  terminated = false;

  addEventListener(type: string, listener: (event: never) => void): void {
    this.#events.addEventListener(type, listener as EventListener);
  }

  postMessage(message: unknown): void {
    this.sent.push(message);
  }

  terminate(): void {
    this.terminated = true;
  }

  answer(data: unknown): void {
    this.#events.dispatchEvent(new MessageEvent('message', { data }));
  }
}

/** The observers made, so that a test can say what came into view. */
const observers: { callback: IntersectionObserverCallback }[] = [];

class FakeObserver {
  constructor(callback: IntersectionObserverCallback) {
    observers.push({ callback });
  }
  readonly observe = vi.fn();
  readonly disconnect = vi.fn();
}

const report = () =>
  ({
    sheets: vi.fn<Report['sheets']>(),
    hidden: vi.fn<Report['hidden']>(),
    shown: vi.fn<Report['shown']>(),
    page: vi.fn<Report['page']>(),
    failed: vi.fn<Report['failed']>(),
    download: vi.fn<Report['download']>(),
  }) satisfies Report;

const LABELS = {
  chart: 'Charts are not shown in the preview',
  pageTooLarge: 'This page is too large to show.',
  download: 'Download',
  hidden: 'Hidden',
};
const SIZES = [
  { width: 800, height: 1100 },
  { width: 800, height: 1100 },
];
const DRAWN = { kind: 'page', svg: '<svg xmlns="http://www.w3.org/2000/svg"/>', text: [] };

/** Tells the view's observer that `target` came into view, or left it. */
function sight(target: Element | null | undefined, isIntersecting: boolean): void {
  observers[0]?.callback(
    [{ target, isIntersecting } as unknown as IntersectionObserverEntry],
    {} as IntersectionObserver,
  );
}

describe('the pages of an HWP document, once shown', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal('IntersectionObserver', FakeObserver);
    observers.length = 0;
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('a page the worker takes too long over stops it, and the viewer is told', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker, 1000);
    const told = report();
    const place = document.createElement('main');
    new HwpView(client, SIZES, place, told, LABELS);
    sight(place.querySelector('[data-page="2"]'), true);
    expect(worker.sent).toEqual([{ kind: 'page', page: 2 }]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(worker.terminated).toBe(true);
    expect(told.failed).toHaveBeenCalledWith('slow');
  });

  it('a page past what the frame takes on says so in its place, and the others are drawn', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker, 1000);
    const told = report();
    const place = document.createElement('main');
    new HwpView(client, SIZES, place, told, LABELS);
    const [first, second] = [...place.querySelectorAll('.page')];
    sight(second, true);
    worker.answer({ kind: 'failed', reason: 'too-large' });
    await vi.advanceTimersByTimeAsync(10);
    expect(second?.querySelector('.page-notice p')?.textContent).toBe(LABELS.pageTooLarge);
    expect(told.failed).not.toHaveBeenCalled();
    expect(worker.terminated).toBe(false);
    second?.querySelector('button')?.click();
    expect(told.download).toHaveBeenCalledOnce();
    sight(first, true);
    worker.answer(DRAWN);
    await vi.advanceTimersByTimeAsync(10);
    expect(first?.querySelector('svg')).not.toBeNull();
    // Out of view and back: the notice again, and the worker is not asked to draw it again.
    sight(second, false);
    expect(second?.children).toHaveLength(0);
    sight(second, true);
    expect(second?.querySelector('.page-notice')).not.toBeNull();
    expect(worker.sent).toEqual([
      { kind: 'page', page: 2 },
      { kind: 'page', page: 1 },
    ]);
  });

  it('a page that left the view before it was drawn is not reported', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker, 1000);
    const told = report();
    const place = document.createElement('main');
    new HwpView(client, SIZES, place, told, LABELS);
    const [first, second] = [...place.querySelectorAll('.page')];
    sight(first, true);
    sight(second, true);
    sight(second, false);
    worker.answer(DRAWN);
    await vi.advanceTimersByTimeAsync(10);
    // Page 2 was given up before it was sent: only page 1 went to the worker.
    expect(worker.sent).toEqual([{ kind: 'page', page: 1 }]);
    expect(first?.querySelector('svg')).not.toBeNull();
    expect(told.failed).not.toHaveBeenCalled();
  });
});

describe('the renderer of HWP documents', () => {
  it('fails, its worker never started, when the app handed it no engine', async () => {
    const worker = vi.fn(() => new FakeWorker());
    const render = renderer('hwp', null, { worker, limit: 30_000 });
    const opening = render(new ArrayBuffer(4), document.createElement('main'), report(), LABELS);
    await expect(opening).rejects.toThrow('no engine');
    expect(worker).not.toHaveBeenCalled();
  });

  it('stops its worker when the frame goes (pagehide), even while it opens a file', async () => {
    const worker = new FakeWorker();
    const render = renderer('hwpx', new ArrayBuffer(1), { worker: () => worker, limit: 30_000 });
    const opening = render(new ArrayBuffer(4), document.createElement('main'), report(), LABELS);
    await vi.waitFor(() => {
      expect(worker.sent).toHaveLength(1);
    });
    window.dispatchEvent(new PageTransitionEvent('pagehide'));
    expect(worker.terminated).toBe(true);
    await expect(opening).rejects.toBeInstanceOf(WorkerClosed);
  });

  it('lets its worker take as long as it takes until the document is shown, and gives each page its time after', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('IntersectionObserver', FakeObserver);
    observers.length = 0;
    try {
      const worker = new FakeWorker();
      const render = renderer('hwp', new ArrayBuffer(1), { worker: () => worker, limit: 1000 });
      const told = report();
      const place = document.createElement('main');
      const opening = render(new ArrayBuffer(4), place, told, LABELS);
      expect(worker.sent).toHaveLength(1);
      // The app keeps the time of the opening, and asks the reader whether to wait on.
      await vi.advanceTimersByTimeAsync(5000);
      expect(worker.terminated).toBe(false);
      worker.answer({ kind: 'opened', pages: SIZES });
      await vi.advanceTimersByTimeAsync(0);
      expect(worker.sent).toEqual([expect.anything(), { kind: 'page', page: 1 }]);
      // The second page comes into view meanwhile: it is asked for once the first is drawn.
      sight(place.querySelector('[data-page="2"]'), true);
      await vi.advanceTimersByTimeAsync(5000);
      expect(worker.terminated).toBe(false);
      worker.answer(DRAWN);
      await expect(opening).resolves.toBeDefined();
      expect(told.shown).toHaveBeenCalledWith(2);
      expect(worker.sent.at(-1)).toEqual({ kind: 'page', page: 2 });
      // Under way as the document was shown: its time counts from then.
      await vi.advanceTimersByTimeAsync(999);
      expect(worker.terminated).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      expect(worker.terminated).toBe(true);
      expect(told.failed).toHaveBeenCalledWith('slow');
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it('shows a document whose first page is too large to draw, that page saying so', async () => {
    vi.stubGlobal('IntersectionObserver', FakeObserver);
    const worker = new FakeWorker();
    const render = renderer('hwp', new ArrayBuffer(1), { worker: () => worker, limit: 30_000 });
    const told = report();
    const place = document.createElement('main');
    const opening = render(new ArrayBuffer(4), place, told, LABELS);
    await vi.waitFor(() => {
      expect(worker.sent).toHaveLength(1);
    });
    worker.answer({ kind: 'opened', pages: SIZES });
    await vi.waitFor(() => {
      expect(worker.sent).toHaveLength(2);
    });
    worker.answer({ kind: 'failed', reason: 'too-large' });
    await expect(opening).resolves.toBeDefined();
    expect(told.shown).toHaveBeenCalledWith(2);
    expect(place.querySelector('[data-page="1"] .page-notice')).not.toBeNull();
    expect(worker.terminated).toBe(false);
    vi.unstubAllGlobals();
  });
});
