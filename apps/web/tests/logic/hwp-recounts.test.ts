/**
 * What a save counts otherwise than the document had it
 * (editor-frame/recounts.ts): the pages of the file opened and of that file
 * saved and read back, both in the worker; rhwp's list of controls tallied by
 * kind (viewer-frame/hwp-controls.ts), of the document in the editor and of
 * its saved file read back; and the names the app gives the kinds
 * ($lib/file-viewer/control-names.ts).
 */
import { HwpDocument } from '@rhwp/core';
import { describe, expect, it } from 'vitest';

import { leftOut, openedOf } from '../../src/editor-frame/comparison.ts';
import { notAccepted, recountsOf } from '../../src/editor-frame/recounts.ts';
import { controlName } from '../../src/lib/file-viewer/control-names.ts';
import {
  CONTROL_COUNT_LIMIT,
  RECOUNT_LIMIT,
  readFromEditor,
} from '../../src/lib/file-viewer/editor-protocol.ts';
import { PAGE_LIMIT } from '../../src/lib/file-viewer/protocol.ts';
import { tallyControls, type ControlTally } from '../../src/viewer-frame/hwp-controls.ts';
import { HwpReader } from '../../src/viewer-frame/hwp-document.ts';
import { rhwpWasmBytes } from '../../tools/build/rhwp-wasm.ts';
import { koreanHwp, koreanHwpx, pageNumbersTwiceHwpx, startRhwp } from '../support/hwp-files.ts';

const WASM = rhwpWasmBytes().buffer;

const kinds = (controls: Record<string, number>): ControlTally =>
  Object.entries(controls).sort(([one], [other]) => (one < other ? -1 : 1));

const copyOf = (data: Uint8Array): ArrayBuffer => {
  const bytes = new ArrayBuffer(data.byteLength);
  new Uint8Array(bytes).set(data);
  return bytes;
};

/** The file read back in a reader of its own, as the frame reads a saved file, and counted. */
async function readBack(format: 'hwp' | 'hwpx', data: Uint8Array): Promise<ControlTally> {
  const reader = new HwpReader();
  const opened = await reader.answer({ kind: 'open', format, bytes: copyOf(data), wasm: WASM });
  const counted = await reader.answer({ kind: 'controls' });
  if (opened.kind !== 'opened' || counted.kind !== 'controls') throw new Error('not read back');
  return counted.controls;
}

/** The file opened in the worker, and its pages there and saved as rhwp saves it, read back. */
async function resaved(
  format: 'hwp' | 'hwpx',
  data: Uint8Array,
): Promise<{ reader: HwpReader; pages: readonly [number, number] }> {
  const reader = new HwpReader();
  const opened = await reader.answer({ kind: 'open', format, bytes: copyOf(data), wasm: WASM });
  const counted = await reader.answer({ kind: 'resaved' });
  if (opened.kind !== 'opened' || counted.kind !== 'resaved') throw new Error('not resaved');
  expect(counted.pages).toBe(opened.pages.length);
  return { reader, pages: [counted.pages, counted.saved] };
}

describe('the controls of a document, tallied', () => {
  it('are counted by kind, in the order of their ids', () => {
    const list = [{ ctrlId: 'tbl' }, { ctrlId: 'secd' }, { ctrlId: 'tbl' }, { ctrlId: '%hlk' }];
    expect(tallyControls(list)).toEqual([
      ['%hlk', 1],
      ['secd', 1],
      ['tbl', 2],
    ]);
  });

  it('count an id that is none as a kind not known, and refuse a list that is none', () => {
    expect(
      tallyControls([
        { ctrlId: 'tbl\u0000' },
        { ctrlId: 'table' },
        { ctrlId: '' },
        { ctrlId: 'tbl ' },
      ]),
    ).toEqual([['', 4]]);
    for (const wrong of [undefined, {}, [{}], [{ ctrlId: 7 }], [null], 'tbl']) {
      expect(() => tallyControls(wrong), JSON.stringify(wrong)).toThrow();
    }
  });
});

describe('what the saved file counts otherwise', () => {
  it('is nothing where the counts are the same', () => {
    expect(recountsOf([3, 3], kinds({ tbl: 1, gso: 2 }), kinds({ gso: 2, tbl: 1 }))).toEqual([]);
    // What was not counted is not asked about; what was, still is.
    expect(recountsOf(undefined, kinds({ pgnp: 3 }), kinds({ pgnp: 2 }))).toEqual([
      { of: 'controls', id: 'pgnp', before: 3, after: 2 },
    ]);
    expect(recountsOf([157, 159], kinds({ pgnp: 3 }), undefined)).toEqual([
      { of: 'pages', before: 157, after: 159 },
    ]);
    expect(recountsOf([157, 159], undefined, kinds({ pgnp: 2 }))).toEqual([
      { of: 'pages', before: 157, after: 159 },
    ]);
  });

  it('is the pages first, then each kind that differs by its id, a kind gone or new as 0', () => {
    expect(
      recountsOf(
        [157, 159],
        kinds({ pgnp: 3, tbl: 1, gso: 2 }),
        kinds({ pgnp: 2, gso: 2, eqed: 1 }),
      ),
    ).toEqual([
      { of: 'pages', before: 157, after: 159 },
      { of: 'controls', id: 'eqed', before: 0, after: 1 },
      { of: 'controls', id: 'pgnp', before: 3, after: 2 },
      { of: 'controls', id: 'tbl', before: 1, after: 0 },
    ]);
  });

  it('is asked about again only where it is not what the writer saved a version with', () => {
    const pages = { of: 'pages', before: 157, after: 159 } as const;
    const pgnp = { of: 'controls', id: 'pgnp', before: 2, after: 1 } as const;
    expect(notAccepted([pages, pgnp], [])).toEqual([pages, pgnp]);
    expect(notAccepted([pages, pgnp], [pgnp])).toEqual([pages]);
    expect(notAccepted([pages, pgnp], [pages, pgnp])).toEqual([]);
    // Another difference is asked about.
    expect(notAccepted([{ ...pages, after: 160 }], [pages])).toEqual([{ ...pages, after: 160 }]);
    expect(notAccepted([{ ...pgnp, id: 'head' }], [pgnp])).toEqual([{ ...pgnp, id: 'head' }]);
    expect(notAccepted([{ ...pgnp, before: 3 }], [pgnp])).toEqual([{ ...pgnp, before: 3 }]);
  });

  it('is told within the protocol’s limits, which the app reads', () => {
    const many = Object.fromEntries(
      Array.from({ length: RECOUNT_LIMIT + 5 }, (_, at) => [`k${String(at)}`, 1]),
    );
    const recounts = recountsOf(
      [PAGE_LIMIT + 2, 1],
      kinds({ tbl: CONTROL_COUNT_LIMIT + 1, ...many }),
      kinds({ tbl: 1 }),
    );
    expect(recounts).toHaveLength(RECOUNT_LIMIT);
    expect(recounts[0]).toEqual({ of: 'pages', before: PAGE_LIMIT, after: 1 });
    // Counts past the limit are the limit: two such counts are one.
    expect(recountsOf([PAGE_LIMIT + 1, PAGE_LIMIT + 2], [], [])).toEqual([]);
    // No page is no count of pages, which the app would refuse.
    expect(recountsOf([3, 0], [], [])).toEqual([]);
    const message = {
      kind: 'exported',
      request: 1,
      bytes: new ArrayBuffer(1),
      count: 0,
      losses: [],
      unprotected: false,
      recounts,
    };
    expect(readFromEditor(message)).toEqual(message);
  });
});

describe('a document saved unchanged, or with a picture deleted, read back', () => {
  for (const [format, data] of [
    ['hwp', koreanHwp()],
    ['hwpx', koreanHwpx()],
  ] as const) {
    it(`counts as the editor’s document does (${format})`, async () => {
      const { pages } = await resaved(format, data);
      expect(pages[1]).toBe(pages[0]);
      startRhwp();
      const document = new HwpDocument(new Uint8Array(data));
      try {
        const export_ = () =>
          format === 'hwp' ? document.exportHwpWithReport() : document.exportHwpxWithReport();
        const now = (): ControlTally => tallyControls(JSON.parse(document.getControls()));
        const unchanged = export_().takeBytes();
        expect(recountsOf(pages, now(), await readBack(format, unchanged))).toEqual([]);

        // The picture deleted in the editor: its data is still written, and the counts agree.
        const list = JSON.parse(document.getControls()) as {
          ctrlId: string;
          list: number;
          para: number;
          controlIndex: number;
        }[];
        const picture = list.find((control) => control.ctrlId === 'gso');
        if (!picture) throw new Error('the sample has no picture');
        document.deleteControlAt(picture.list, picture.para, picture.controlIndex);
        const edited = export_();
        const bytes = edited.takeBytes();
        expect(JSON.parse(edited.contentLoss())).toMatchObject({ count: 0 });
        expect(leftOut(openedOf(format, new Uint8Array(data)), bytes).losses).toEqual([]);
        const after = await readBack(format, bytes);
        expect(after).not.toContainEqual(['gso', 1]);
        expect(recountsOf(pages, now(), after)).toEqual([]);
      } finally {
        document.free();
      }
    });
  }
});

describe('a document whose saved file reads back otherwise', () => {
  it('is counted as it is in the editor, and as its saved file reads back', async () => {
    const data = pageNumbersTwiceHwpx();
    const { reader, pages } = await resaved('hwpx', data);
    // The saved file took the document's place in the worker.
    const counted = await reader.answer({ kind: 'controls' });
    if (counted.kind !== 'controls') throw new Error('not counted');
    expect(counted.controls).toContainEqual(['pgnp', 1]);
    startRhwp();
    const document = new HwpDocument(new Uint8Array(data));
    try {
      const before = tallyControls(JSON.parse(document.getControls()));
      expect(before).toContainEqual(['pgnp', 2]);
      const exported = document.exportHwpxWithReport();
      expect(JSON.parse(exported.contentLoss())).toMatchObject({ count: 0 });
      const after = await readBack('hwpx', exported.takeBytes());
      expect(recountsOf(pages, before, after)).toEqual([
        { of: 'controls', id: 'pgnp', before: 2, after: 1 },
      ]);
    } finally {
      document.free();
    }
  });

  it('is not counted in a worker with no document open', async () => {
    expect(await new HwpReader().answer({ kind: 'resaved' })).toEqual({
      kind: 'failed',
      reason: 'renderer',
    });
  });
});

describe('the name of a kind of control', () => {
  it('is the app’s where it knows the kind, and its id otherwise', () => {
    expect(controlName('pgnp')).toBe('Page number position');
    expect(controlName('tbl')).toBe('Table');
    expect(controlName('%hlk')).toBe('Hyperlink');
    expect(controlName('%xyz')).toBe('Field ‘%xyz’');
    expect(controlName('zzzz')).toBe('Control ‘zzzz’');
    expect(controlName('')).toBe('Control the editor does not recognize');
    expect(controlName('toString')).toBe('Control ‘toString’');
  });
});
