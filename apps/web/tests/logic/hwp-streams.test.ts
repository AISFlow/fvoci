/**
 * What a save leaves out of an HWP document, which rhwp's report of the save
 * does not name (editor-frame/streams.ts): a stream found by its path, a piece
 * of data by what it holds, the preview Hangul makes again, and the
 * protection of a distribution document (배포용 문서) taken away, told once.
 */
import { HwpDocument } from '@rhwp/core';
import { describe, expect, it } from 'vitest';

import { leftOut, openedOf } from '../../src/editor-frame/comparison.ts';
import { lossesOf } from '../../src/editor-frame/losses.ts';
import { hwpStreams, streamsLeftOut } from '../../src/editor-frame/streams.ts';
import { readFromEditor } from '../../src/lib/file-viewer/editor-protocol.ts';
import { HwpReader } from '../../src/viewer-frame/hwp-document.ts';
import {
  compoundFile,
  compoundStreams,
  DISTRIBUTED_TEXT,
  distributionHwp,
  hwpFileHeader,
  koreanHwp,
  OTHER_STREAM,
  startRhwp,
  withStreams,
  withViewTextLeft,
  type CompoundStream,
} from '../support/hwp-files.ts';

/** The bytes as the editor is given them: checked, and written anew. */
async function checked(data: Uint8Array): Promise<Uint8Array> {
  const bytes = new ArrayBuffer(data.byteLength);
  new Uint8Array(bytes).set(data);
  const answer = await new HwpReader().answer({ kind: 'check', format: 'hwp', bytes });
  if (answer.kind !== 'checked') throw new Error(`refused: ${JSON.stringify(answer)}`);
  return new Uint8Array(answer.bytes);
}

/** What rhwp saves of the document, unchanged, its report, and the text it reads in it. */
function saved(given: Uint8Array): { bytes: Uint8Array; reported: number; text: string } {
  startRhwp();
  const document = new HwpDocument(given);
  try {
    const exported = document.exportHwpWithReport();
    try {
      const { count } = lossesOf(JSON.parse(exported.contentLoss()));
      return { bytes: exported.takeBytes(), reported: count, text: document.getTextFileUnicode() };
    } finally {
      exported.free();
    }
  } finally {
    document.free();
  }
}

const DISTRIBUTION = 1 << 2;
const COMPRESSED = 1;

/** An HWP's compound file of `streams` after a `FileHeader` with `properties`. */
const hwp = (properties: number, streams: Record<string, string>): Uint8Array =>
  compoundFile([
    { path: 'FileHeader', data: hwpFileHeader(properties) },
    ...Object.entries(streams).map(([path, data]): CompoundStream => ({
      path,
      data: Buffer.from(data),
    })),
  ]);

const paths = (given: Uint8Array, kept: Uint8Array): string[] =>
  streamsLeftOut(hwpStreams(given), hwpStreams(kept)).losses.map((loss) => loss.path);

describe('the streams a save leaves out of an HWP', () => {
  it('are none of a document rhwp keeps whole, nor of what it saved itself', async () => {
    const given = await checked(koreanHwp());
    const once = saved(given);
    expect(once.reported).toBe(0);
    expect(streamsLeftOut(hwpStreams(given), hwpStreams(once.bytes))).toEqual({
      unprotected: false,
      losses: [],
    });
    const again = await checked(once.bytes);
    expect(streamsLeftOut(hwpStreams(again), hwpStreams(saved(again).bytes)).losses).toEqual([]);
  });

  it('are the streams rhwp does not keep, which its report does not name', async () => {
    // A protected body left in a document that is no distribution document: rhwp reads the
    // open one, and does not keep this.
    const given = await checked(withViewTextLeft());
    const once = saved(given);
    expect(once.reported).toBe(0);
    expect(once.text).not.toContain(DISTRIBUTED_TEXT);
    expect(streamsLeftOut(hwpStreams(given), hwpStreams(once.bytes))).toEqual({
      unprotected: false,
      losses: [
        {
          code: 'partOmitted',
          subject: 'stream',
          reason: 'notKeptByEditor',
          path: 'ViewText/Section0',
        },
      ],
    });
    // A stream of another program's rhwp keeps as it is.
    const other = await checked(withStreams(koreanHwp(), [OTHER_STREAM]));
    expect(streamsLeftOut(hwpStreams(other), hwpStreams(saved(other).bytes)).losses).toEqual([]);
  });

  it('are found by path ignoring case, and in any order', () => {
    const given = hwp(COMPRESSED, { 'Scripts/DefaultJScript': 'x', DocInfo: 'y', 'Custom/a': 'z' });
    expect(paths(given, hwp(COMPRESSED, { DOCINFO: '1', 'SCRIPTS/defaultjscript': '2' }))).toEqual([
      'Custom/a',
    ]);
    expect(
      streamsLeftOut(hwpStreams(given), hwpStreams(hwp(COMPRESSED, { DocInfo: '1' }))).losses,
    ).toEqual(
      ['Custom/a', 'Scripts/DefaultJScript'].map((path) => ({
        code: 'partOmitted',
        subject: 'stream',
        reason: 'notKeptByEditor',
        path,
      })),
    );
  });

  it('are not an empty stream, which holds nothing to lose', () => {
    expect(
      paths(hwp(COMPRESSED, { DocInfo: 'y', Empty: '' }), hwp(COMPRESSED, { DocInfo: 'y' })),
    ).toEqual([]);
  });

  it('tell the preview Hangul makes again apart from what is lost', () => {
    const given = hwp(COMPRESSED, { PrvImage: 'picture', PrvText: 'text', DocInfo: 'y' });
    const { losses } = streamsLeftOut(
      hwpStreams(given),
      hwpStreams(hwp(COMPRESSED, { DocInfo: 'y' })),
    );
    expect(losses.map((loss) => [loss.path, loss.reason])).toEqual([
      ['PrvImage', 'previewMadeAgain'],
      ['PrvText', 'previewMadeAgain'],
    ]);
  });

  it('find a piece of data by what it holds, each piece once', () => {
    const given = hwp(COMPRESSED, {
      'BinData/BIN0001.png': 'picture',
      'BinData/BIN0002.png': 'picture',
    });
    expect(
      paths(
        given,
        hwp(COMPRESSED, { 'BinData/BIN0007.png': 'picture', 'BinData/BIN0008.png': 'picture' }),
      ),
    ).toEqual([]);
    expect(paths(given, hwp(COMPRESSED, { 'BinData/BIN0001.png': 'picture' }))).toEqual([
      'BinData/BIN0002.png',
    ]);
    // Data written otherwise, or emptied, is not what was given.
    expect(
      paths(
        given,
        hwp(COMPRESSED, { 'BinData/BIN0001.png': 'picture', 'BinData/BIN0002.png': '' }),
      ),
    ).toEqual(['BinData/BIN0002.png']);
  });

  it('are no save of bytes that are no HWP', () => {
    const given = hwpStreams(hwp(COMPRESSED, { DocInfo: 'y' }));
    expect(() => streamsLeftOut(given, hwpStreams(new Uint8Array(600)))).toThrow();
    expect(() =>
      streamsLeftOut(
        given,
        hwpStreams(compoundFile([{ path: 'DocInfo', data: Buffer.from('y') }])),
      ),
    ).toThrow();
  });
});

describe('a distribution document saved', () => {
  it('is read from its protected body, and saved as an open one, told once', async () => {
    const given = await checked(distributionHwp());
    expect(compoundStreams(Buffer.from(given)).map((stream) => stream.path)).toContain(
      'ViewText/Section0',
    );
    const once = saved(given);
    // rhwp read the protected body: the open one does not have this text.
    expect(once.text).toContain(DISTRIBUTED_TEXT);
    expect(once.reported).toBe(0);
    const savedPaths = compoundStreams(Buffer.from(once.bytes)).map((stream) => stream.path);
    expect(savedPaths).not.toContain('ViewText/Section0');
    expect(hwpStreams(once.bytes).distribution).toBe(false);

    const opened = openedOf('hwp', given);
    const { unprotected, losses, kept } = leftOut(opened, once.bytes);
    expect({ unprotected, losses }).toEqual({ unprotected: true, losses: [] });
    // What is kept of the saved file is what a later save is compared with: no distribution
    // document, so nothing is said of protection again.
    expect(leftOut(kept, once.bytes)).toMatchObject({ unprotected: false, losses: [] });
    const message = {
      kind: 'exported',
      request: 1,
      bytes: new ArrayBuffer(1),
      count: 0,
      losses: [],
      unprotected: true,
      recounts: [],
    };
    expect(readFromEditor(message)).toEqual(message);
    expect(readFromEditor({ ...message, unprotected: 'yes' })).toBeUndefined();
    expect(readFromEditor({ ...message, unprotected: undefined })).toBeUndefined();
  });

  it('keeps its protection where the saved file is still one, and says the rest as streams', () => {
    const given = hwp(COMPRESSED | DISTRIBUTION, { 'ViewText/Section0': 'x', PrvImage: 'p' });
    expect(hwpStreams(given).distribution).toBe(true);
    // Still a distribution document with its protected body: nothing to say of protection.
    expect(
      streamsLeftOut(
        hwpStreams(given),
        hwpStreams(hwp(COMPRESSED | DISTRIBUTION, { 'ViewText/Section0': 'y', PrvImage: 'p' })),
      ),
    ).toEqual({ unprotected: false, losses: [] });
    // Said to be one, without its protected body: unprotected, the body not said again.
    const bare = streamsLeftOut(hwpStreams(given), hwpStreams(hwp(COMPRESSED | DISTRIBUTION, {})));
    expect(bare.unprotected).toBe(true);
    expect(bare.losses.map((loss) => loss.path)).toEqual(['PrvImage']);
    // A document that is no distribution document loses `ViewText` as any stream.
    const plain = hwp(COMPRESSED, { 'ViewText/Section0': 'x' });
    expect(streamsLeftOut(hwpStreams(plain), hwpStreams(hwp(COMPRESSED, {})))).toEqual({
      unprotected: false,
      losses: [expect.objectContaining({ path: 'ViewText/Section0' })],
    });
  });
});
