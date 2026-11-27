import { deflateRawSync } from 'node:zlib';

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { compoundFile as readCompound } from '$lib/file-viewer/compound-file.ts';
import { examine, type FormatId } from '$lib/file-viewer/formats.ts';
import { readFromFrame } from '$lib/file-viewer/protocol.ts';
import { checkCompound } from '../../src/viewer-frame/compound-guard.ts';
import { HwpReader } from '../../src/viewer-frame/hwp-document.ts';
import type { FromWorker, HwpFormat } from '../../src/viewer-frame/hwp-messages.ts';
import { SlowRenderer } from '../../src/viewer-frame/renderer.ts';
import {
  WorkerClient,
  WorkerClosed,
  WorkerFailed,
  type WorkerLike,
} from '../../src/viewer-frame/worker-client.ts';

import {
  compoundFile,
  compoundStreams,
  denseHwpx,
  HWP_TEXT,
  hwpFileHeader,
  koreanHwp,
  koreanHwpx,
  lockedHwp,
  lockedHwpx,
  rewriteArchive,
} from '../support/hwp-files.ts';
import { zip } from '../support/viewer-files.ts';
import { rhwpWasmBytes } from '../../tools/build/rhwp-wasm.ts';

const bytes = (buffer: Buffer): Uint8Array =>
  new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
const buffer = (data: Buffer): ArrayBuffer => Uint8Array.from(data).buffer;
const sniffFormat = (data: Buffer): FormatId | undefined => {
  const examined = examine(bytes(data));
  return examined.kind === 'format' ? examined.format : undefined;
};

const MIMETYPE = Buffer.from('application/hwp+zip');
/** An HWPX archive of a `mimetype` part and a section, the part stored or not. */
const hwpxArchive = (mimetype: Buffer, stored = true): Buffer =>
  zip([
    { name: 'mimetype', data: mimetype, stored },
    { name: 'Contents/section0.xml', data: Buffer.from('<hs:sec/>') },
  ]);
const SECTION = { name: 'Contents/section0.xml', data: Buffer.from('<hs:sec/>') };
const LIST = { name: 'Contents/content.hpf', data: Buffer.from('<opf:package/>') };

describe('HWPX is told apart by its parts', () => {
  it('HWPX: a ZIP archive whose stored mimetype is application/hwp+zip', () => {
    expect(sniffFormat(koreanHwpx())).toBe('hwpx');
    expect(sniffFormat(hwpxArchive(MIMETYPE))).toBe('hwpx');
  });

  it('a mimetype that is deflated, another type, or missing is no HWPX by itself', () => {
    expect(sniffFormat(hwpxArchive(MIMETYPE, false))).toBeUndefined();
    expect(sniffFormat(hwpxArchive(Buffer.from('application/epub+zip')))).toBeUndefined();
    expect(sniffFormat(hwpxArchive(Buffer.from('application/hwp+zip\n')))).toBeUndefined();
    expect(sniffFormat(zip([SECTION]))).toBeUndefined();
  });

  it('HWPX also: an archive with the list of its parts, whatever its mimetype', () => {
    // As writers other than Hancom's make them: the mimetype deflated, or left out.
    const deflated = { name: 'mimetype', data: MIMETYPE, stored: false };
    expect(sniffFormat(zip([deflated, LIST, SECTION]))).toBe('hwpx');
    expect(sniffFormat(zip([LIST, SECTION]))).toBe('hwpx');
    // A Word document that has such a part is still a Word document.
    const word = { name: 'word/document.xml', data: Buffer.from('<w:document/>') };
    expect(sniffFormat(zip([word, LIST]))).toBe('docx');
  });

  it('an HWPX whose writer left the mimetype out is opened as any other', async () => {
    const without = rewriteArchive(koreanHwpx(), (member) =>
      member.name === 'mimetype' ? undefined : member,
    );
    expect(sniffFormat(without)).toBe('hwpx');
    const opened = await open(new HwpReader(), 'hwpx', without);
    expect(opened).toMatchObject({ kind: 'opened' });
  });
});

/** rhwp's WebAssembly, as the build hands it to the frame. */
const WASM = rhwpWasmBytes().buffer;

const open = (reader: HwpReader, format: HwpFormat, data: Buffer): Promise<FromWorker> =>
  reader.answer({ kind: 'open', format, bytes: buffer(data), wasm: WASM });

describe("the worker's reader, with rhwp", () => {
  it.each([
    ['hwpx', koreanHwpx],
    ['hwp', koreanHwp],
  ] as const)('opens the Korean %s sample of two pages and draws them', async (format, make) => {
    const reader = new HwpReader();
    const opened = await open(reader, format, make());
    expect(opened.kind).toBe('opened');
    if (opened.kind !== 'opened') return;
    expect(opened.pages).toHaveLength(2);
    expect(opened.pages[0]?.width).toBeCloseTo(793.7, 0);
    const first = await reader.answer({ kind: 'page', page: 1 });
    if (first.kind !== 'page') throw new Error(`page 1: ${JSON.stringify(first)}`);
    expect(first.svg.startsWith('<svg')).toBe(true);
    expect(first.svg).toContain('data:image/png;base64,');
    const lines = first.text.map((run) => run.text);
    expect(lines).toContain(HWP_TEXT.header);
    expect(lines).toContain(HWP_TEXT.title);
    expect(lines).toContain(HWP_TEXT.body);
    for (const cell of ['항목', '담당', '김민지']) expect(lines).toContain(cell);
    const second = await reader.answer({ kind: 'page', page: 2 });
    expect(second.kind === 'page' && second.text.map((run) => run.text)).toContain(HWP_TEXT.second);
    expect(await reader.answer({ kind: 'page', page: 3 })).toEqual({
      kind: 'failed',
      reason: 'renderer',
    });
  });

  it('opens the HWP sample written again as version 4, and with DIFAT sectors', async () => {
    const streams = compoundStreams(koreanHwp());
    const picture = { path: 'BinData/BIN0009.bmp', data: Buffer.alloc(8 << 20, 9) };
    for (const again of [
      compoundFile(streams, { version: 4 }),
      compoundFile([...streams, picture]),
    ]) {
      const opened = await open(new HwpReader(), 'hwp', again);
      expect(opened.kind === 'opened' && opened.pages.length).toBe(2);
    }
  });

  it('draws an HWP of 12 MiB, whose directory rhwp finds through the DIFAT of the file written anew', async () => {
    const filler = { path: 'BinData/BIN0099.dat', data: Buffer.alloc(12 << 20, 9) };
    const large = compoundFile([...compoundStreams(koreanHwp()), filler]);
    // The sectors the header's own list of FAT sectors covers: 109 of 128 numbers each.
    const listed = 109 * 128;
    const written = readCompound(new Uint8Array(checkCompound(buffer(large))));
    expect(written?.difatSectors.length).toBeGreaterThan(0);
    expect(written?.directorySectors[0]).toBeGreaterThan(listed);
    expect(written?.entries[0]?.start).toBeGreaterThan(listed);
    const reader = new HwpReader();
    const opened = await open(reader, 'hwp', large);
    expect(opened.kind === 'opened' && opened.pages.length).toBe(2);
    const first = await reader.answer({ kind: 'page', page: 1 });
    if (first.kind !== 'page') throw new Error(`page 1: ${JSON.stringify(first)}`);
    expect(first.svg).toContain('data:image/png;base64,');
    expect(first.text.map((run) => run.text)).toContain(HWP_TEXT.title);
  });

  it('stops a picture that unpacks past the limit, as written or read leniently by rhwp', async () => {
    const streams = compoundStreams(koreanHwp());
    const bomb = deflateRawSync(Buffer.alloc(4 << 20));
    const bombed = streams.map((stream) =>
      stream.path.startsWith('BinData/') ? { ...stream, data: bomb } : stream,
    );
    expect(bombed).not.toEqual(streams);
    const strict = compoundFile(bombed);
    // An unused entry of the directory with a kind that does not exist: rhwp's compound file
    // reader refuses it, and rhwp reads the file leniently, pictures unpacked whole.
    const lenient = Buffer.from(strict);
    const directory = (strict.readUInt32LE(48) + 1) * 512;
    const unused = readCompound(bytes(strict))?.entries.findIndex((entry) => entry.type === 0);
    if (unused === undefined || unused < 0) throw new Error('no unused entry');
    lenient[directory + unused * 128 + 66] = 3;
    for (const file of [strict, lenient]) {
      expect(await open(new HwpReader({ unpacked: 1 << 20 }), 'hwp', file)).toEqual({
        kind: 'failed',
        reason: 'too-large',
      });
    }
  });

  it('does not hand on a page past the most text or elements a page may be', async () => {
    for (const limits of [{ svg: 1000 }, { elements: 50 }]) {
      const reader = new HwpReader(limits);
      expect((await open(reader, 'hwpx', koreanHwpx())).kind).toBe('opened');
      expect(await reader.answer({ kind: 'page', page: 1 })).toEqual({
        kind: 'failed',
        reason: 'too-large',
      });
    }
    const reader = new HwpReader({ elements: 5000 });
    expect((await open(reader, 'hwpx', koreanHwpx())).kind).toBe('opened');
    expect((await reader.answer({ kind: 'page', page: 1 })).kind).toBe('page');
  });

  it('takes a limit given as undefined for the ceiling, not for none', async () => {
    const reader = new HwpReader({ svg: undefined, elements: undefined, unpacked: undefined });
    expect((await open(reader, 'hwpx', denseHwpx())).kind).toBe('opened');
    expect(await reader.answer({ kind: 'page', page: 1 })).toEqual({
      kind: 'failed',
      reason: 'too-large',
    });
    expect((await reader.answer({ kind: 'page', page: 2 })).kind).toBe('page');
  });

  it('says a document locked with a password is locked', async () => {
    expect(await open(new HwpReader(), 'hwpx', lockedHwpx())).toEqual({
      kind: 'failed',
      reason: 'locked',
    });
    expect(await open(new HwpReader(), 'hwp', lockedHwp())).toEqual({
      kind: 'failed',
      reason: 'locked',
    });
  });

  it('says a file it cannot read is unreadable', async () => {
    // An archive with the HWPX media type and nothing else: counted, then refused by rhwp.
    expect(await open(new HwpReader(), 'hwpx', hwpxArchive(MIMETYPE))).toEqual({
      kind: 'failed',
      reason: 'unreadable',
    });
    // A compound file with an HWP header and nothing else: checked, then refused by rhwp.
    const empty = compoundFile([{ path: 'FileHeader', data: hwpFileHeader() }]);
    expect(await open(new HwpReader(), 'hwp', empty)).toEqual({
      kind: 'failed',
      reason: 'unreadable',
    });
  });
});

/** A worker that answers when told to, and records what it was sent. */
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

  fail(): void {
    this.#events.dispatchEvent(new Event('error'));
  }
}

describe("the frame's client of a renderer's worker", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('sends one request at a time, the next once the one before is answered', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker, 1000);
    const first = client.request('one');
    const second = client.request('two');
    expect(worker.sent).toEqual(['one']);
    worker.answer(1);
    await expect(first).resolves.toBe(1);
    expect(worker.sent).toEqual(['one', 'two']);
    worker.answer(2);
    await expect(second).resolves.toBe(2);
  });

  it('a request given up before it is sent is not sent', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker, 1000);
    const first = client.request('one');
    const giveUp = new AbortController();
    const second = client.request('two', [], giveUp.signal);
    const third = client.request('three');
    giveUp.abort();
    worker.answer(1);
    await first;
    await expect(second).rejects.toThrow('Given up');
    expect(worker.sent).toEqual(['one', 'three']);
    worker.answer(3);
    await expect(third).resolves.toBe(3);
  });

  it('stops the worker when a request takes too long, and fails what waits', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker, 1000);
    const first = client.request('one');
    const second = client.request('two');
    vi.advanceTimersByTime(999);
    expect(worker.terminated).toBe(false);
    vi.advanceTimersByTime(1);
    expect(worker.terminated).toBe(true);
    await expect(first).rejects.toBeInstanceOf(SlowRenderer);
    await expect(second).rejects.toBeInstanceOf(SlowRenderer);
    await expect(client.request('three')).rejects.toBeInstanceOf(SlowRenderer);
    expect(client.closed).toBeInstanceOf(SlowRenderer);
  });

  it('the time counts from when a request is sent, not from when it was asked', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker, 1000);
    const first = client.request('one');
    const second = client.request('two');
    vi.advanceTimersByTime(900);
    worker.answer(1);
    await first;
    vi.advanceTimersByTime(900);
    expect(worker.terminated).toBe(false);
    worker.answer(2);
    await expect(second).resolves.toBe(2);
  });

  it('made with no time, waits as long as it takes, and once given one holds what is under way to it from then', async () => {
    const worker = new FakeWorker();
    const client = new WorkerClient(worker, undefined);
    const first = client.request('one');
    const second = client.request('two');
    vi.advanceTimersByTime(60_000);
    expect(worker.terminated).toBe(false);
    worker.answer(1);
    await expect(first).resolves.toBe(1);
    vi.advanceTimersByTime(60_000);
    client.limitEach(1000);
    vi.advanceTimersByTime(999);
    expect(worker.terminated).toBe(false);
    worker.answer(2);
    await expect(second).resolves.toBe(2);
    // The next has its time from when it is sent.
    const third = client.request('three');
    vi.advanceTimersByTime(1000);
    expect(worker.terminated).toBe(true);
    await expect(third).rejects.toBeInstanceOf(SlowRenderer);
    // A client stopped stays stopped.
    client.limitEach(1000);
    expect(client.closed).toBeInstanceOf(SlowRenderer);
  });

  it('closing stops the worker; a worker that fails on its own is stopped too', async () => {
    const closing = new FakeWorker();
    const client = new WorkerClient(closing, 1000);
    const waiting = client.request('one');
    client.close();
    expect(closing.terminated).toBe(true);
    await expect(waiting).rejects.toBeInstanceOf(WorkerClosed);

    const failing = new FakeWorker();
    const other = new WorkerClient(failing, 1000);
    const pending = other.request('one');
    failing.fail();
    expect(failing.terminated).toBe(true);
    await expect(pending).rejects.toBeInstanceOf(WorkerFailed);
  });
});

describe('the frame says when its renderer took too long', () => {
  it('as a failure the app takes', () => {
    expect(readFromFrame({ kind: 'failed', reason: 'slow' })).toEqual({
      kind: 'failed',
      reason: 'slow',
    });
  });
});
