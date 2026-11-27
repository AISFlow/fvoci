import { constants, deflateRawSync, deflateSync } from 'node:zlib';

import { describe, expect, it } from 'vitest';

import { compoundFile, verifyChains } from '$lib/file-viewer/compound-file.ts';
import { examine, UNPACKED_LIMIT_BYTES, type FormatId } from '$lib/file-viewer/formats.ts';
import { checkCompound } from '../../src/viewer-frame/compound-guard.ts';
import { LockedFile, TooLargeFile, UnreadableFile } from '../../src/viewer-frame/renderer.ts';

import {
  compoundFile as writeCompound,
  compoundStreams,
  hwpFileHeader,
  koreanHwp,
  lockedHwp,
  type CompoundStream,
} from '../support/hwp-files.ts';
import { zip } from '../support/viewer-files.ts';

const bytes = (buffer: Buffer): Uint8Array =>
  new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
const buffer = (data: Buffer): ArrayBuffer => Uint8Array.from(data).buffer;
const sniffFormat = (data: Buffer): FormatId | undefined => {
  const examined = examine(bytes(data));
  return examined.kind === 'format' ? examined.format : undefined;
};

/** An HWP document of a header and a section, with what the streams say about themselves. */
const hwpFile = (section: Partial<CompoundStream> = {}, cycle?: string): Buffer =>
  writeCompound(
    [
      { path: 'FileHeader', data: hwpFileHeader(1) },
      { path: 'DocInfo', data: Buffer.alloc(8000, 1) },
      { path: 'BodyText/Section0', data: Buffer.alloc(9000, 2), ...section },
    ],
    { cycle },
  );

describe('HWP is told apart by its first bytes and its streams', () => {
  it('HWP: a compound file whose FileHeader starts with "HWP Document File"', () => {
    expect(sniffFormat(koreanHwp())).toBe('hwp');
    expect(sniffFormat(hwpFile())).toBe('hwp');
  });

  it('another compound file (an old Word or Excel file) is none of them', () => {
    const word = writeCompound([
      { path: 'WordDocument', data: Buffer.alloc(5000, 7) },
      { path: '\u0001CompObj', data: Buffer.alloc(100, 1) },
    ]);
    const excel = writeCompound([{ path: 'Workbook', data: Buffer.alloc(300, 3) }]);
    expect(compoundFile(bytes(word))).toBeDefined();
    expect(sniffFormat(word)).toBeUndefined();
    expect(sniffFormat(excel)).toBeUndefined();
  });

  it('a FileHeader with another signature, or inside a storage, is no HWP', () => {
    const other = Buffer.from(hwpFileHeader());
    other.write('HWP Document Fill', 'ascii');
    expect(sniffFormat(writeCompound([{ path: 'FileHeader', data: other }]))).toBeUndefined();
    const nested = writeCompound([{ path: 'Hidden/FileHeader', data: hwpFileHeader() }]);
    expect(sniffFormat(nested)).toBeUndefined();
  });

  it('the signature of a compound file alone, or a broken one, is nothing', () => {
    const signature = Buffer.from([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    expect(sniffFormat(Buffer.concat([signature, Buffer.alloc(2000)]))).toBeUndefined();
    const broken = hwpFile();
    broken.writeUInt32LE(0xfffffff0, 48); // the directory's first sector
    expect(sniffFormat(broken)).toBeUndefined();
  });
});

describe('the app answers "too large" from what the streams say, before anything unpacks', () => {
  it('together up to 200 MB; past that it is offered as a download', () => {
    // The header and DocInfo say their own 256 + 8000 bytes; the section says the rest.
    const at = hwpFile({ said: UNPACKED_LIMIT_BYTES - 8256 });
    const past = hwpFile({ said: UNPACKED_LIMIT_BYTES - 8255 });
    expect(examine(bytes(at))).toEqual({ kind: 'format', format: 'hwp' });
    expect(examine(bytes(past))).toEqual({ kind: 'too-large', format: 'hwp' });
  });

  it('many streams that share one chain count each time', () => {
    const streams: CompoundStream[] = [
      { path: 'FileHeader', data: hwpFileHeader() },
      { path: 'BinData/BIN0000.png', data: Buffer.alloc(1 << 20, 5) },
    ];
    for (let index = 1; index < 200; index += 1) {
      const name = `BinData/BIN${String(index).padStart(4, '0')}.png`;
      streams.push({
        path: name,
        data: Buffer.alloc(4096),
        sharesWith: 'BinData/BIN0000.png',
        said: 1 << 20,
      });
    }
    const shared = writeCompound(streams);
    expect(shared.length).toBeLessThan(4 << 20);
    expect(examine(bytes(shared))).toEqual({ kind: 'too-large', format: 'hwp' });
  });
});

describe('in the frame, a compound file is checked before rhwp reads it', () => {
  /** The streams of a file, each path with its bytes, in order of path. */
  const listed = (data: Buffer): string[][] =>
    compoundStreams(data)
      .map((stream) => [stream.path, stream.data.toString('hex')])
      .sort();

  it('an honest one is handed on written anew, with the same streams', () => {
    const honest = koreanHwp();
    const written = Buffer.from(checkCompound(buffer(honest)));
    expect(listed(written)).toEqual(listed(honest));
    expect(checkCompound(buffer(hwpFile()))).toBeInstanceOf(ArrayBuffer);
  });

  it('what rhwp would read by rules of its own is written as it reads it here', () => {
    // The high half of a version 3 size (which only a lenient reader takes), and an unused
    // entry of a kind that does not exist (which sends rhwp down its lenient reader).
    const honest = koreanHwp();
    const odd = Buffer.from(honest);
    const file = compoundFile(bytes(odd));
    const picture = file?.entries.findIndex((entry) => entry.name.startsWith('BIN'));
    const unused = file?.entries.findIndex((entry) => entry.type === 0);
    if (!file || picture === undefined || picture < 0 || unused === undefined || unused < 0) {
      throw new Error('no picture or no unused entry');
    }
    const at = (index: number): number => (odd.readUInt32LE(48) + 1) * 512 + index * 128;
    odd.writeUInt32LE(1, at(picture) + 124);
    odd[at(unused) + 66] = 3;
    const written = Buffer.from(checkCompound(buffer(odd)));
    expect(listed(written)).toEqual(listed(honest));
    const again = compoundFile(bytes(written));
    expect(again?.entries.every((entry) => [0, 1, 2, 5].includes(entry.type))).toBe(true);
  });

  it('a storage that holds a name twice, ignoring case, stops the file', () => {
    const twice = writeCompound([
      { path: 'FileHeader', data: hwpFileHeader() },
      { path: 'BodyText/Section0', data: Buffer.alloc(100, 1) },
      { path: 'BodyText/Section1', data: Buffer.alloc(100, 2) },
    ]);
    const file = compoundFile(bytes(twice));
    const second = file?.entries.findIndex((entry) => entry.name === 'Section1') ?? -1;
    const at = (twice.readUInt32LE(48) + 1) * 512 + second * 128;
    twice.write('s', at + 0, 'utf16le');
    twice.write('0', at + 14, 'utf16le');
    expect(compoundFile(bytes(twice))?.entries[second]?.name).toBe('section0');
    expect(() => checkCompound(buffer(twice))).toThrow(UnreadableFile);
  });

  it('thousands of small streams are read in a moment: the mini stream is read once', () => {
    // Read again for each stream, 4,000 of them took 82 seconds, past the worker's time.
    const streams: CompoundStream[] = [{ path: 'FileHeader', data: hwpFileHeader() }];
    for (let index = 0; index < 4000; index += 1) {
      streams.push({ path: `BinData/B${String(index)}`, data: Buffer.alloc(4095, index & 0xff) });
    }
    const many = writeCompound(streams);
    const started = performance.now();
    expect(checkCompound(buffer(many), 1 << 30)).toBeInstanceOf(ArrayBuffer);
    expect(performance.now() - started).toBeLessThan(10_000);
  }, 60_000);

  it('a stream that says more than its chain holds stops the file', () => {
    expect(() => checkCompound(buffer(hwpFile({ said: 20_000 })))).toThrow(UnreadableFile);
  });

  it('two streams on one chain, or a chain that comes back on itself, stop the file', () => {
    const shared = writeCompound([
      { path: 'FileHeader', data: hwpFileHeader() },
      { path: 'BodyText/Section0', data: Buffer.alloc(9000, 2) },
      { path: 'BodyText/Section1', data: Buffer.alloc(9000, 3), sharesWith: 'BodyText/Section0' },
    ]);
    const file = compoundFile(bytes(shared));
    expect(file && verifyChains(file)).toBe(false);
    expect(() => checkCompound(buffer(shared))).toThrow(UnreadableFile);
    const cycle = hwpFile({}, 'BodyText/Section0');
    expect(() => checkCompound(buffer(cycle))).toThrow(UnreadableFile);
  });

  it('a document locked with a password says so', () => {
    const locked = writeCompound([{ path: 'FileHeader', data: hwpFileHeader(0b11) }]);
    expect(() => checkCompound(buffer(locked))).toThrow(LockedFile);
    expect(() => checkCompound(buffer(lockedHwp()))).toThrow(LockedFile);
  });
});

describe('in the frame, what the streams unpack to is held to the limit', () => {
  const LIMIT = 1 << 20;
  /** An HWP document with a picture stream of these bytes. */
  const withPicture = (...pictures: Buffer[]): ArrayBuffer =>
    buffer(
      writeCompound([
        { path: 'FileHeader', data: hwpFileHeader(1) },
        { path: 'DocInfo', data: deflateRawSync(Buffer.alloc(8000, 1)) },
        ...pictures.map((data, index) => ({ path: `BinData/BIN000${String(index)}.bmp`, data })),
      ]),
    );
  const zeros = Buffer.alloc(4 << 20);

  it('a picture deflated past it, raw or in zlib, stops the file', () => {
    expect(() => checkCompound(withPicture(deflateRawSync(zeros)), LIMIT)).toThrow(TooLargeFile);
    expect(() => checkCompound(withPicture(deflateSync(zeros)), LIMIT)).toThrow(TooLargeFile);
  });

  it('what comes out before a stream fails counts: with a byte after it, or a broken block', () => {
    const part = Buffer.alloc(600 << 10);
    const junk = Buffer.concat([deflateRawSync(part), Buffer.from([0x42])]);
    // Blocks that do not end the stream, then one of a kind that does not exist.
    const open = deflateRawSync(part, { finishFlush: constants.Z_SYNC_FLUSH });
    const broken = Buffer.concat([open, Buffer.from([0x07, 0, 0, 0])]);
    expect(checkCompound(withPicture(junk), LIMIT)).toBeInstanceOf(ArrayBuffer);
    expect(() => checkCompound(withPicture(junk, broken), LIMIT)).toThrow(TooLargeFile);
  });

  it('bytes after the last block, as Hancom writes them, take nothing from the count', () => {
    // A browser's unpacker fails on them, and drops what the slice they were in unpacked to.
    const after = (size: number): Buffer =>
      Buffer.concat([deflateRawSync(Buffer.alloc(size)), Buffer.from([0x07, 0x00])]);
    expect(after(2 << 20).length).toBeLessThan(16 * 1024);
    expect(() => checkCompound(withPicture(after(2 << 20)), LIMIT)).toThrow(TooLargeFile);
    expect(checkCompound(withPicture(after(LIMIT - 9000)), LIMIT)).toBeInstanceOf(ArrayBuffer);
    expect(() => checkCompound(withPicture(after(LIMIT - 8000)), LIMIT)).toThrow(TooLargeFile);
  });

  it('a stream that is not deflated counts as it is, and the streams count together', () => {
    const stored = Buffer.alloc(600 << 10, 0x89);
    expect(checkCompound(withPicture(stored), LIMIT)).toBeInstanceOf(ArrayBuffer);
    expect(() => checkCompound(withPicture(stored, stored), LIMIT)).toThrow(TooLargeFile);
    expect(
      checkCompound(withPicture(deflateRawSync(Buffer.alloc(900 << 10))), LIMIT),
    ).toBeInstanceOf(ArrayBuffer);
  });
});

describe('a broken compound file is nothing, not an error', () => {
  /** Where entry `index` of the directory starts, in a file written by the test writer. */
  const entryAt = (file: Buffer, index: number): number => {
    const size = 1 << file.readUInt16LE(30);
    return (file.readUInt32LE(48) + 1) * size + index * 128;
  };

  it('a FAT sector past the end of the file, or a DIFAT chain that ends too soon', () => {
    const far = hwpFile();
    far.writeUInt32LE(0x7fffffff, 76);
    expect(compoundFile(bytes(far))).toBeUndefined();
    const picture = { path: 'BinData/BIN0009.bmp', data: Buffer.alloc(8 << 20, 9) };
    const large = writeCompound([...compoundStreams(koreanHwp()), picture]);
    expect(compoundFile(bytes(large))).toBeDefined();
    large.writeUInt32LE(0xfffffffe, 68);
    expect(compoundFile(bytes(large))).toBeUndefined();
  });

  it('a version 4 file with the high half of a size set', () => {
    const file = writeCompound([{ path: 'FileHeader', data: hwpFileHeader() }], { version: 4 });
    expect(compoundFile(bytes(file))).toBeDefined();
    file.writeUInt32LE(1, entryAt(file, 1) + 124);
    expect(compoundFile(bytes(file))).toBeUndefined();
  });

  it("the root's mini stream: shared with a stream, larger than its chain, or one chain for two", () => {
    const honest = hwpFile();
    const root = entryAt(honest, 0);
    const section = compoundFile(bytes(honest))?.entries.find((entry) => entry.name === 'Section0');
    if (!section) throw new Error('no section');
    const verified = (file: Buffer): boolean | undefined => {
      const read = compoundFile(bytes(file));
      return read && verifyChains(read);
    };
    expect(verified(honest)).toBe(true);
    const sharing = Buffer.from(honest);
    sharing.writeUInt32LE(section.start, root + 116);
    expect(verified(sharing)).toBe(false);
    const larger = Buffer.from(honest);
    larger.writeUInt32LE(1 << 20, root + 120);
    expect(verified(larger)).toBe(false);
    const twoMini = writeCompound([
      { path: 'FileHeader', data: hwpFileHeader() },
      { path: 'DocInfo', data: Buffer.alloc(1000, 1) },
      { path: 'PrvText', data: Buffer.alloc(1000, 2), sharesWith: 'DocInfo' },
    ]);
    expect(verified(twoMini)).toBe(false);
  });
});

describe('an archive or a compound file is told by what it holds, not as a PDF', () => {
  const pdf = Buffer.from('%PDF-1.7\n%%EOF\n');

  it('a compound file with a PDF picture stored early', () => {
    const file = writeCompound([
      { path: 'FileHeader', data: hwpFileHeader() },
      { path: 'BinData/BIN0001.pdf', data: Buffer.concat([pdf, Buffer.alloc(5000)]) },
    ]);
    expect(file.subarray(0, 1024).includes(pdf.subarray(0, 5))).toBe(true);
    expect(sniffFormat(file)).toBe('hwp');
  });

  it('an archive with a PDF part stored early', () => {
    const archive = zip([
      { name: 'mimetype', data: Buffer.from('application/hwp+zip'), stored: true },
      { name: 'BinData/a.pdf', data: pdf, stored: true },
      { name: 'Contents/section0.xml', data: Buffer.from('<hs:sec/>') },
    ]);
    expect(archive.subarray(0, 1024).includes(pdf.subarray(0, 5))).toBe(true);
    expect(sniffFormat(archive)).toBe('hwpx');
  });
});

describe('the reader takes a compound file of either version, and a FAT past the header', () => {
  const listed = (streams: readonly CompoundStream[]): string[][] =>
    streams.map((stream) => [stream.path, stream.data.toString('hex')]).sort();
  const sample = compoundStreams(koreanHwp());

  it('reads every stream of the sample', () => {
    const paths = sample.map((stream) => stream.path);
    for (const path of ['FileHeader', 'DocInfo', 'BodyText/Section0'])
      expect(paths).toContain(path);
  });

  it('version 4 (sectors of 4096 bytes)', () => {
    const again = writeCompound(sample, { version: 4 });
    expect(again.readUInt16LE(26)).toBe(4);
    expect(sniffFormat(again)).toBe('hwp');
    expect(checkCompound(buffer(again))).toBeInstanceOf(ArrayBuffer);
    expect(listed(compoundStreams(again))).toEqual(listed(sample));
  });

  it('a FAT longer than the header lists, continued in DIFAT sectors', () => {
    const picture = { path: 'BinData/BIN0009.bmp', data: Buffer.alloc(8 << 20, 9) };
    const large = writeCompound([...sample, picture]);
    expect(large.readUInt32LE(72)).toBeGreaterThan(0);
    expect(sniffFormat(large)).toBe('hwp');
    expect(checkCompound(buffer(large))).toBeInstanceOf(ArrayBuffer);
    expect(listed(compoundStreams(large))).toEqual(listed([...sample, picture]));
  });
});
