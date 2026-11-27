/**
 * A deck's hidden slides in the viewer's session
 * (src/lib/file-viewer/session.svelte.ts), with a frame that only answers:
 * the deck opens on the first slide a show gives, pages are turned past the
 * slides left out, showing them keeps the reader's slide, leaving them out
 * lands on the next one, a page told before they were left out is not the
 * reader's, and only a deck has hidden slides. And a frame given up: one that
 * did not start, an engine that did not come, each told as its own cause; a
 * file not drawn in its time asks the reader whether to wait on for it.
 */
import { Blob as NodeBlob } from 'node:buffer';

import { afterEach, describe, expect, it, vi } from 'vitest';

import { LIMITS, ViewerSession } from '$lib/file-viewer/session.svelte.ts';

import { koreanHwpx } from '../support/hwp-files.ts';
import { koreanPdf } from '../support/viewer-files.ts';
import { numberedPptx } from '../support/viewer-pptx.ts';

const sessions: ViewerSession[] = [];

afterEach(() => {
  for (const session of sessions.splice(0)) session.close();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

/** A session of `file` with its frame in the page; what it sends, and a way to answer. */
async function mounted(name: string, file: Buffer, engine?: () => Promise<ArrayBuffer>) {
  const session = new ViewerSession(
    {
      name,
      size: file.length,
      read: () => Promise.resolve(new NodeBlob([new Uint8Array(file)]) as Blob),
    },
    {
      theme: () => 'light',
      lang: () => 'en',
      onKey: () => undefined,
      onDownload: () => undefined,
      ...(engine ? { engine } : {}),
    },
  );
  sessions.push(session);
  await vi.waitFor(() => {
    expect(session.status.name).toBe('opening');
  });
  const frame = document.createElement('iframe');
  document.body.append(frame);
  const window_ = frame.contentWindow;
  if (!window_) throw new Error('no frame');
  const sent: unknown[] = [];
  vi.spyOn(window_, 'postMessage').mockImplementation((message: unknown) => {
    sent.push(message);
  });
  session.attach(frame);
  const tell = (data: unknown): void => {
    window.dispatchEvent(new MessageEvent('message', { data, source: window_ }));
  };
  return { session, sent, tell };
}

/** As `mounted`, the frame told it is ready. */
async function opened(name: string, file: Buffer, engine?: () => Promise<ArrayBuffer>) {
  const shown = await mounted(name, file, engine);
  shown.tell({ kind: 'ready' });
  return shown;
}

describe('a frame given up in the session, each cause told apart', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('a frame that does not say it is ready did not start', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const { session } = await mounted('deck.pptx', numberedPptx(3, []));
    await vi.advanceTimersByTimeAsync(LIMITS.ready - 1);
    expect(session.status.name).toBe('opening');
    await vi.advanceTimersByTimeAsync(1);
    expect(session.status).toEqual({ name: 'failed', reason: 'unstarted' });
  });

  it('a frame given up is not heard, nor handed the file, when it says it is ready after all', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const { session, sent, tell } = await mounted('deck.pptx', numberedPptx(3, []));
    await vi.advanceTimersByTimeAsync(LIMITS.ready);
    tell({ kind: 'ready' });
    await vi.advanceTimersByTimeAsync(0);
    expect(session.status).toEqual({ name: 'failed', reason: 'unstarted' });
    expect(sent).toEqual([]);
  });

  it('an engine that does not come, or is refused, is the engine', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const hwp = koreanHwpx();
    let arrive = (_bytes: ArrayBuffer): void => undefined;
    const asked = vi.fn(() => new Promise<ArrayBuffer>((resolve) => (arrive = resolve)));
    const late = await opened('slow.hwpx', hwp, asked);
    await vi.advanceTimersByTimeAsync(LIMITS.engine);
    expect(late.session.status).toEqual({ name: 'failed', reason: 'engine' });
    expect(asked).toHaveBeenCalledTimes(1);
    // An engine that comes after it was given up hands the frame nothing.
    arrive(new ArrayBuffer(8));
    await vi.advanceTimersByTimeAsync(0);
    expect(late.session.status).toEqual({ name: 'failed', reason: 'engine' });
    expect(late.sent).toEqual([]);
    const refused = await opened('gone.hwpx', hwp, () => Promise.reject(new Error('gone')));
    await vi.waitFor(() => {
      expect(refused.session.status).toEqual({ name: 'failed', reason: 'engine' });
    });
    expect(refused.sent).toEqual([]);
  });

  it('a file not drawn in its time asks the reader, and is asked of again a time after waiting on', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const { session, sent, tell } = await opened('deck.pptx', numberedPptx(3, []));
    expect(sent).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(LIMITS.draw - 1);
    expect(session.status).toEqual({ name: 'opening', format: 'pptx', late: false });
    await vi.advanceTimersByTimeAsync(1);
    expect(session.status).toEqual({ name: 'opening', format: 'pptx', late: true });
    // Asked, and not answered: the frame goes on, and nothing more comes of the time.
    await vi.advanceTimersByTimeAsync(LIMITS.draw * 3);
    expect(session.status).toEqual({ name: 'opening', format: 'pptx', late: true });
    session.waitOn();
    expect(session.status).toEqual({ name: 'opening', format: 'pptx', late: false });
    await vi.advanceTimersByTimeAsync(LIMITS.draw);
    expect(session.status).toEqual({ name: 'opening', format: 'pptx', late: true });
    session.waitOn();
    // Drawn at last, in the frame it was handed to: nothing was sent to it again.
    tell({ kind: 'shown', pages: 3 });
    expect(session.status).toEqual({ name: 'shown', format: 'pptx' });
    expect(sent).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(LIMITS.draw * 2);
    expect(session.status.name).toBe('shown');
  });

  it('a file drawn while the reader is asked is shown; one taken instead is given up as slow', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const asked = await opened('deck.pptx', numberedPptx(3, []));
    await vi.advanceTimersByTimeAsync(LIMITS.draw);
    asked.tell({ kind: 'shown', pages: 3 });
    expect(asked.session.status).toEqual({ name: 'shown', format: 'pptx' });
    // Only a question is answered.
    asked.session.waitOn();
    asked.session.giveUp();
    expect(asked.session.status.name).toBe('shown');

    const taken = await opened('deck.pptx', numberedPptx(3, []));
    taken.session.giveUp();
    expect(taken.session.status.name).toBe('opening');
    await vi.advanceTimersByTimeAsync(LIMITS.draw);
    taken.session.giveUp();
    expect(taken.session.status).toEqual({ name: 'failed', reason: 'slow' });
    // What the frame says after it was given up is not heard.
    taken.tell({ kind: 'shown', pages: 3 });
    expect(taken.session.status).toEqual({ name: 'failed', reason: 'slow' });
  });
});

describe("a deck's hidden slides in the session", () => {
  it('first and last: opened on the first a show gives, turned past them both ways', async () => {
    const { session, sent, tell } = await opened('deck.pptx', numberedPptx(6, [1, 2, 6]));
    tell({ kind: 'hidden', pages: [1, 2, 6] });
    tell({ kind: 'shown', pages: 6 });
    expect(session.status.name).toBe('shown');
    expect(session.page).toBe(3);
    expect(session.canTurn(-1)).toBe(false);
    session.turn(1);
    session.turn(1);
    expect(session.page).toBe(5);
    expect(sent.at(-1)).toEqual({ kind: 'page', page: 5 });
    expect(session.canTurn(1)).toBe(false);

    // Shown: the reader stays on 5, and may turn to 6.
    session.toggleHidden();
    expect(session.showHidden).toBe(true);
    expect(sent.at(-1)).toEqual({ kind: 'hidden', shown: true });
    expect(session.page).toBe(5);
    session.turn(1);
    expect(session.page).toBe(6);

    // Left out on 6: back on 5, the last a show gives. A page the frame told of before (6), or
    // any slide left out, is not the reader's; the frame's own landing is.
    session.toggleHidden();
    expect(sent.at(-1)).toEqual({ kind: 'hidden', shown: false });
    expect(session.page).toBe(5);
    tell({ kind: 'page', page: 6 });
    tell({ kind: 'page', page: 2 });
    expect(session.page).toBe(5);
    tell({ kind: 'page', page: 4 });
    expect(session.page).toBe(4);
  });

  it('every slide hidden: all are turned through, and there is nothing to leave out', async () => {
    const { session, sent, tell } = await opened('deck.pptx', numberedPptx(2, [1, 2]));
    tell({ kind: 'hidden', pages: [1, 2] });
    tell({ kind: 'shown', pages: 2 });
    expect(session.page).toBe(1);
    expect(session.slides?.allHidden).toBe(true);
    const before = sent.length;
    session.toggleHidden();
    expect(sent).toHaveLength(before);
    session.turn(1);
    expect(session.page).toBe(2);
  });

  it('only a deck has hidden slides; ones past its pages are none', async () => {
    const pdf = await opened('report.pdf', koreanPdf(3));
    pdf.tell({ kind: 'hidden', pages: [2] });
    pdf.tell({ kind: 'shown', pages: 3 });
    expect(pdf.session.slides).toBeUndefined();
    pdf.session.turn(1);
    expect(pdf.session.page).toBe(2);

    const deck = await opened('deck.pptx', numberedPptx(3, []));
    deck.tell({ kind: 'hidden', pages: [9] });
    deck.tell({ kind: 'shown', pages: 3 });
    expect(deck.session.slides).toBeUndefined();
    expect(deck.session.page).toBe(1);
  });
});
