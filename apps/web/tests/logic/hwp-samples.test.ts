/**
 * Real HWP and HWPX documents through the viewer's check, to see that it
 * refuses none that rhwp reads: run by hand against a folder of samples,
 * which this repository does not hold.
 *
 *   FVOCI_HWP_SAMPLES=<folder> pnpm --filter web exec vitest run --project logic tests/logic/hwp-samples.test.ts
 *
 * Every `.hwp` and `.hwpx` under the folder is opened twice: as the viewer
 * opens it (the check, then rhwp: `HwpReader`), and by rhwp alone. A file
 * rhwp reads and the viewer refuses is a wrong refusal, unless the refusal is
 * one of the viewer's own limits (`too-large`). `FVOCI_HWP_SAMPLES_REPORT`
 * names a file the counts and the files that differ are written to.
 *
 * The samples are not trusted: they are read as bytes and nothing in their
 * folder is run. Each file gets a module of rhwp of its own, so that one
 * that leaves rhwp trapped or full does not decide the next.
 */
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it, vi } from 'vitest';

import { examine, VIEW_LIMIT_BYTES } from '$lib/file-viewer/formats.ts';

import type { FromWorker, HwpFormat } from '../../src/viewer-frame/hwp-messages.ts';
import { rhwpWasmBytes } from '../../tools/build/rhwp-wasm.ts';

const FOLDER = process.env.FVOCI_HWP_SAMPLES;
const REPORT = process.env.FVOCI_HWP_SAMPLES_REPORT;

/** What a document of HWP 3.0 starts with: a file of its own layout, no compound file. */
const HWP3 = 'HWP Document File V3';

/** What became of one sample. */
interface Outcome {
  file: string;
  bytes: number;
  /** What the app takes the bytes for before the frame has them (`examine`). */
  examined: string;
  /** The pages the viewer's reader opened, or why it did not. */
  viewer: number | string;
  /** The pages rhwp alone opened, or what it said. */
  alone: number | string;
  /** Whether the viewer drew the first page of a file it opened. */
  drew?: boolean | string;
}

function samplesUnder(folder: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(folder, { withFileTypes: true })) {
    if (entry.name === '.git') continue;
    const at = path.join(folder, entry.name);
    if (entry.isDirectory()) found.push(...samplesUnder(at));
    else if (/\.hwpx?$/i.test(entry.name) && entry.isFile()) found.push(at);
  }
  return found.sort();
}

const copyOf = (data: Buffer): ArrayBuffer => {
  const copy = new ArrayBuffer(data.byteLength);
  new Uint8Array(copy).set(data);
  return copy;
};

const said = (error: unknown): string =>
  (error instanceof Error ? `${error.name}: ${error.message}` : String(error)).slice(0, 120);

async function tryOne(file: string, wasm: ArrayBuffer): Promise<Outcome> {
  const data = readFileSync(file);
  const outcome: Outcome = {
    file,
    bytes: data.byteLength,
    examined: 'unknown',
    viewer: 'not opened',
    alone: 'not opened',
  };
  const examined = examine(new Uint8Array(data));
  outcome.examined = examined.kind === 'format' ? examined.format : examined.kind;
  // HWP 3.0 is a format of its own, which the viewer does not show: no refusal of the check's.
  if (examined.kind === 'unknown' && data.subarray(0, HWP3.length).toString('latin1') === HWP3) {
    outcome.examined = 'hwp3';
  }
  const format: HwpFormat | undefined =
    outcome.examined === 'hwp' || outcome.examined === 'hwpx' ? outcome.examined : undefined;
  if (data.byteLength > VIEW_LIMIT_BYTES || !format) {
    outcome.viewer = data.byteLength > VIEW_LIMIT_BYTES ? 'too-large' : outcome.examined;
  }

  vi.resetModules();
  const { HwpReader } = await import('../../src/viewer-frame/hwp-document.ts');
  if (format && data.byteLength <= VIEW_LIMIT_BYTES) {
    const reader = new HwpReader();
    try {
      const opened: FromWorker = await reader.answer({
        kind: 'open',
        format,
        bytes: copyOf(data),
        wasm,
      });
      outcome.viewer = opened.kind === 'opened' ? opened.pages.length : failureOf(opened);
      if (opened.kind === 'opened') {
        const page = await reader.answer({ kind: 'page', page: 1 });
        outcome.drew = page.kind === 'page' ? true : failureOf(page);
      }
    } catch (error) {
      outcome.viewer = `trapped (${said(error)})`;
    }
  }

  // rhwp alone, in a module of its own: the viewer's may be trapped.
  vi.resetModules();
  const alone = await import('@rhwp/core');
  try {
    await alone.default({ module_or_path: wasm });
    const document = new alone.HwpDocument(new Uint8Array(copyOf(data)));
    outcome.alone = document.pageCount();
    document.free();
  } catch (error) {
    outcome.alone = said(error);
  }
  return outcome;
}

const failureOf = (answer: FromWorker): string =>
  answer.kind === 'failed' ? answer.reason : answer.kind;

describe.skipIf(!FOLDER)('real documents through the viewer’s check', () => {
  it(
    'refuses none that rhwp reads, and opens them to the same pages',
    async () => {
      const folder = FOLDER ?? '';
      expect(statSync(folder).isDirectory(), folder).toBe(true);
      const wasm = rhwpWasmBytes().buffer;
      const outcomes: Outcome[] = [];
      for (const file of samplesUnder(folder)) outcomes.push(await tryOne(file, wasm));

      const opened = (value: number | string): value is number => typeof value === 'number';
      const relative = (one: Outcome): Outcome => ({
        ...one,
        file: path.relative(folder, one.file),
      });
      const wrongRefusals = outcomes
        .filter((one) => opened(one.alone) && !opened(one.viewer) && one.viewer !== 'too-large')
        .filter((one) => one.examined !== 'hwp3')
        .map(relative);
      const otherPages = outcomes
        .filter((one) => opened(one.alone) && opened(one.viewer) && one.alone !== one.viewer)
        .map(relative);
      const notDrawn = outcomes.filter((one) => one.drew !== undefined && one.drew !== true);
      const counts = new Map<string, number>();
      for (const one of outcomes) {
        const key = [
          one.examined,
          opened(one.viewer) ? 'viewer opens' : `viewer: ${one.viewer}`,
          opened(one.alone) ? 'rhwp opens' : 'rhwp refuses',
        ].join(' | ');
        counts.set(key, (counts.get(key) ?? 0) + 1);
      }
      const summary = {
        files: outcomes.length,
        bytes: outcomes.reduce((sum, one) => sum + one.bytes, 0),
        pages: outcomes.reduce((sum, one) => sum + (opened(one.viewer) ? one.viewer : 0), 0),
        counts: Object.fromEntries([...counts].sort()),
        wrongRefusals,
        otherPages,
        notDrawn: notDrawn.map(relative),
        onlyTheViewerOpens: outcomes
          .filter((one) => opened(one.viewer) && !opened(one.alone))
          .map(relative),
      };
      if (REPORT) writeFileSync(REPORT, `${JSON.stringify(summary, null, 2)}\n`);
      // eslint-disable-next-line no-console -- run by hand: the counts are what it is run for.
      console.log(JSON.stringify({ ...summary, wrongRefusals: wrongRefusals.length }, null, 2));

      expect(outcomes.length, 'the folder holds samples').toBeGreaterThan(0);
      expect(wrongRefusals, 'files rhwp reads and the check refuses').toEqual([]);
      expect(otherPages, 'files the check changes the pages of').toEqual([]);
      // A first page over the viewer's own limit is the limit's doing; any other failure is not.
      const failedPages = notDrawn.filter((one) => one.drew !== 'too-large').map(relative);
      expect(failedPages, 'opened files whose first page is not drawn').toEqual([]);
    },
    60 * 60 * 1000,
  );
});
