/**
 * What a save leaves out of an HWPX package, which rhwp's report of the save
 * does not name (editor-frame/parts.ts): a part found by its name, a piece of
 * data by what it holds, each told in the frame's message as rhwp's losses
 * are.
 */
import { HwpDocument } from '@rhwp/core';
import { describe, expect, it } from 'vitest';

import { lossesOf } from '../../src/editor-frame/losses.ts';
import { packageParts, partsLeftOut, withPartsLeftOut } from '../../src/editor-frame/parts.ts';
import {
  LOSS_LIST_LIMIT,
  LOSS_PATH_LIMIT,
  readFromEditor,
  type Loss,
} from '../../src/lib/file-viewer/editor-protocol.ts';
import { HwpReader } from '../../src/viewer-frame/hwp-document.ts';
import { koreanHwpx, koreanHwpxWithOtherParts, startRhwp } from '../support/hwp-files.ts';
import { zip } from '../support/viewer-files.ts';

/** The bytes as the editor is given them: checked, and written anew. */
async function checked(data: Uint8Array): Promise<Uint8Array> {
  const bytes = new ArrayBuffer(data.byteLength);
  new Uint8Array(bytes).set(data);
  const answer = await new HwpReader().answer({ kind: 'check', format: 'hwpx', bytes });
  if (answer.kind !== 'checked') throw new Error(`refused: ${JSON.stringify(answer)}`);
  return new Uint8Array(answer.bytes);
}

/** What rhwp saves of the document, unchanged, and its report, as the editor asks for them. */
function saved(given: Uint8Array): { bytes: Uint8Array; reported: number } {
  startRhwp();
  const document = new HwpDocument(given);
  try {
    const exported = document.exportHwpxWithReport();
    try {
      const { count } = lossesOf(JSON.parse(exported.contentLoss()));
      return { bytes: exported.takeBytes(), reported: count };
    } finally {
      exported.free();
    }
  } finally {
    document.free();
  }
}

const leftOut = (given: Uint8Array, bytes: Uint8Array): string[] =>
  partsLeftOut(packageParts(given), packageParts(bytes)).map((loss) => loss.path);

const archive = (parts: Record<string, string>): Uint8Array =>
  zip(Object.entries(parts).map(([name, data]) => ({ name, data: Buffer.from(data) })));

describe('the parts a save leaves out of an HWPX package', () => {
  it('are the parts rhwp does not read, which its report does not name', async () => {
    const given = await checked(koreanHwpxWithOtherParts());
    const { bytes, reported } = saved(given);
    expect(reported).toBe(0);
    const left = partsLeftOut(packageParts(given), packageParts(bytes));
    expect(left).toEqual(
      ['Contents/history.xml', 'Custom/notes.xml'].map((path) => ({
        code: 'partOmitted',
        subject: 'packagePart',
        reason: 'notKeptByEditor',
        path,
      })),
    );
    // The data the manifest names is kept, under another name.
    expect(packageParts(bytes).map((part) => part.name)).not.toContain('BinData/attachment.pdf');
  });

  it('are none of a document rhwp keeps whole, nor of what it saved itself', async () => {
    const given = await checked(koreanHwpx());
    const once = saved(given).bytes;
    expect(leftOut(given, once)).toEqual([]);
    const again = await checked(once);
    expect(leftOut(again, saved(again).bytes)).toEqual([]);
  });

  it('find a piece of data by what it holds, and folders not at all', () => {
    const given = archive({
      'BinData/': '',
      'BinData/a.png': 'picture',
      'BinData/b.png': 'picture',
    });
    expect(
      leftOut(given, archive({ 'BinData/image7.png': 'picture', 'BinData/image8.png': 'picture' })),
    ).toEqual([]);
    // Each piece is found once: one picture's worth for two is one left out.
    expect(leftOut(given, archive({ 'BinData/a.png': 'picture' }))).toEqual(['BinData/b.png']);
    // Data written otherwise is not what was given.
    const drawn = archive({ 'BinData/a.png': 'picture', 'BinData/c.png': 'drawing' });
    expect(
      leftOut(drawn, archive({ 'BinData/a.png': 'emptied', 'BinData/c.png': 'drawing' })),
    ).toEqual(['BinData/a.png']);
  });

  it('are no save of bytes that are no archive', () => {
    expect(() => partsLeftOut([], packageParts(new Uint8Array([1, 2, 3])))).toThrow();
  });

  it('go into the frame’s message as the app reads it, however many and however named', () => {
    const many: Record<string, string> = { [`Custom/${'긴'.repeat(400)}.xml`]: 'x' };
    for (let at = 0; at < 250; at += 1) many[`Custom/part${String(at)}.xml`] = 'x';
    const left = partsLeftOut(
      packageParts(archive(many)),
      packageParts(archive({ mimetype: 'x' })),
    );
    expect(left).toHaveLength(251);
    expect(left[0]?.path).toHaveLength(LOSS_PATH_LIMIT);
    const rhwps: Loss = {
      code: 'binaryContentEmptied',
      subject: 'binaryData',
      reason: 'resourceReadFailedOrLimitExceeded',
      path: 'BinData/image1.png',
    };
    for (const report of [
      { count: 3, losses: [rhwps, rhwps, rhwps] },
      { count: 0, losses: [] },
    ]) {
      for (const parts of [left, left.slice(0, 2), []]) {
        const { count, losses } = withPartsLeftOut(report, parts);
        expect(count).toBe(report.count + parts.length);
        expect(losses).toHaveLength(Math.min(count, LOSS_LIST_LIMIT));
        expect(losses.slice(0, Math.min(parts.length, 2))).toEqual(parts.slice(0, 2));
        const bytes = new ArrayBuffer(1);
        const message = {
          kind: 'exported',
          request: 1,
          bytes,
          count,
          losses,
          unprotected: false,
          recounts: [],
        };
        expect(readFromEditor(message)).toEqual(message);
      }
    }
  });

  it('are told once where rhwp’s report names them already', () => {
    const emptied: Loss = {
      code: 'binaryContentEmptied',
      subject: 'binaryData',
      reason: 'storedCompressionMismatch',
      path: '/BinData/BIN0001.png',
    };
    const found = (path: string): Loss => ({
      code: 'partOmitted',
      subject: 'stream',
      reason: 'notKeptByEditor',
      path,
    });
    // The picture rhwp wrote empty is no longer found by what it held: the report says it.
    expect(
      withPartsLeftOut({ count: 1, losses: [emptied] }, [
        found('BinData/bin0001.png'),
        found('Custom/a'),
      ]),
    ).toEqual({ count: 2, losses: [found('Custom/a'), emptied] });
    // A part of a package is named as written: one spelled otherwise is another part.
    const part = (path: string): Loss => ({ ...found(path), subject: 'packagePart' });
    const named = { ...emptied, path: 'BinData/image1.png' };
    expect(
      withPartsLeftOut({ count: 1, losses: [named] }, [
        part('BinData/image1.png'),
        part('BinData/Image1.png'),
      ]),
    ).toEqual({ count: 2, losses: [part('BinData/Image1.png'), named] });
  });
});
