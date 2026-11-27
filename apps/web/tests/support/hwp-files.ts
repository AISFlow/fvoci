/**
 * Files for the tests of the HWP viewer, made here rather than kept as
 * binaries: an HWP and an HWPX document in Korean with a header, a table, a
 * picture and a second page, written by rhwp's own exporter (`@rhwp/core`,
 * the viewer's renderer, run in Node), and a writer of compound files (OLE)
 * that can tell the lies the viewer must catch. Every file is ours, so the
 * tests carry no third-party content.
 */
import { createCipheriv } from 'node:crypto';
import { inflateRawSync } from 'node:zlib';

import { HwpDocument, initSync } from '@rhwp/core';

import {
  chain,
  childrenOf,
  COMPOUND_TYPES,
  compoundFile as readCompound,
  readStream,
  type CompoundEntry,
  type CompoundFile,
} from '../../src/lib/file-viewer/compound-file.ts';
import { zipEntries } from '../../src/lib/file-viewer/zip-directory.ts';
import { writeCompound } from '../../src/viewer-frame/compound-writer.ts';

import { rhwpWasmBytes } from '../../tools/build/rhwp-wasm.ts';

import { png, zip, type ZipMember } from './viewer-files.ts';

/** The text of the sample, for the tests to look for. */
export const HWP_TEXT = {
  header: '머리글: 회의록',
  title: '분기 회의록',
  body: '이 문서는 한국어 견본입니다. 표와 그림이 들어 있습니다.',
  cells: ['항목', '담당', '기한', '설계 검토', '김민지', '11월 20일'],
  second: '둘째 쪽: 다음 회의 안건',
};

/** The password of the locked samples. */
export const HWP_PASSWORD = '비밀1234';

let ready = false;

/**
 * Starts rhwp from the WebAssembly the build hands the frame, its memory held
 * to the same limit: rhwp's loader keeps the first instance it is given, so the
 * worker's reader, tried in the same module context, runs on this one.
 */
export function startRhwp(): void {
  if (ready) return;
  initSync({ module: rhwpWasmBytes() });
  ready = true;
}

function done(result: string): void {
  const answer = JSON.parse(result) as { ok?: boolean };
  if (!answer.ok) throw new Error(`rhwp: ${result}`);
}

/** HWPUNITs (1/7200 inch) per CSS pixel. */
const UNITS_PER_PIXEL = 75;

/** The sample, two pages, as rhwp holds it. */
function koreanDocument(): HwpDocument {
  startRhwp();
  const document = HwpDocument.createEmpty();
  document.createBlankDocument();
  // A paragraph per line: the title, the body, the table's, the picture's, the second page's.
  const lines = [HWP_TEXT.title, HWP_TEXT.body, '', '', HWP_TEXT.second];
  lines.forEach((line, index) => {
    if (line) done(document.insertText(0, index, 0, line));
    if (index < lines.length - 1) done(document.splitParagraph(0, index, line.length));
  });
  done(document.createTable(0, 2, 0, 2, 3));
  HWP_TEXT.cells.forEach((cell, index) => {
    done(document.insertTextInCell(0, 2, 0, index, 0, 0, cell));
  });
  // Placed below the table, from the paper's corner: without an offset rhwp puts it in the corner.
  const [x, y] = [113 * UNITS_PER_PIXEL, 240 * UNITS_PER_PIXEL];
  const picture = png(200, 100, [46, 125, 210]);
  done(document.insertPicture(0, 3, 0, '', picture, 15000, 7500, 200, 100, 'png', '그림', x, y));
  done(document.createHeaderFooter(0, true, 0));
  done(document.insertTextInHeaderFooter(0, true, 0, 0, 0, HWP_TEXT.header));
  done(document.insertPageBreak(0, 4, 0));
  return document;
}

/**
 * The part rhwp adds to an HWPX it writes from its own document, which makes
 * it read the file back with the layout rules of HWP. Hancom's editor drops it
 * on saving; the samples drop it too, to be read as any HWPX is.
 */
const RHWP_ORIGIN = 'META-INF/rhwp-hwp5-origin';

/**
 * The archive again with each part as `change` makes it (the same, another,
 * or left out when it gives undefined), each stored or deflated as it was.
 */
export function rewriteArchive(
  archive: Buffer,
  change: (member: ZipMember) => ZipMember | undefined,
): Buffer {
  const members = (zipEntries(archive) ?? []).flatMap((entry) => {
    const at = entry.offset;
    const start = at + 30 + archive.readUInt16LE(at + 26) + archive.readUInt16LE(at + 28);
    const packed = archive.subarray(start, start + entry.compressedSize);
    const stored = entry.method === 0;
    const changed = change({
      name: entry.name,
      data: stored ? packed : inflateRawSync(packed),
      stored,
    });
    return changed ? [changed] : [];
  });
  return zip(members);
}

export function koreanHwpx(): Buffer {
  const exported = Buffer.from(koreanDocument().exportHwpx());
  return rewriteArchive(exported, (member) => (member.name === RHWP_ORIGIN ? undefined : member));
}

/** The place of the page numbers at the foot of the page, as Hangul writes it in a section. */
const PAGE_NUMBER =
  '<hp:ctrl><hp:pageNum pos="BOTTOM_CENTER" formatType="DIGIT" sideChar="-"/></hp:ctrl>';

/**
 * The Korean HWPX with the place of its page numbers set twice in its first
 * paragraph, as Hangul can write it: rhwp keeps one, and the saved file reads
 * back with one place of page numbers where the document in the editor had
 * two, which its report does not say.
 */
export function pageNumbersTwiceHwpx(): Buffer {
  return rewriteArchive(koreanHwpx(), (member) => {
    if (member.name !== 'Contents/section0.xml') return member;
    const section = member.data.toString('utf8');
    const at = section.indexOf('</hp:ctrl>') + '</hp:ctrl>'.length;
    if (at < '</hp:ctrl>'.length) throw new Error('the section has no control');
    const twice = `${section.slice(0, at)}${PAGE_NUMBER}${PAGE_NUMBER}${section.slice(at)}`;
    return { ...member, data: Buffer.from(twice) };
  });
}

/** Parts rhwp does not keep, and a piece of data it keeps under a name of its own. */
const OTHER_PARTS: ZipMember[] = [
  {
    name: 'Contents/history.xml',
    data: Buffer.from(
      '<?xml version="1.0" encoding="UTF-8"?><hhs:history xmlns:hhs="http://www.hancom.co.kr/hwpml/2011/history"/>',
    ),
  },
  { name: 'Custom/notes.xml', data: Buffer.from('<?xml version="1.0"?><notes>기록</notes>') },
  { name: 'BinData/attachment.pdf', data: Buffer.from('%PDF-1.4 attached') },
];
/** How the package's manifest names the history and the data; the notes it does not name. */
const OTHER_ITEMS =
  '<opf:item id="history" href="Contents/history.xml" media-type="application/xml"/>' +
  '<opf:item id="attachment" href="BinData/attachment.pdf" media-type="application/pdf" isEmbeded="1"/>';

/**
 * The Korean HWPX with parts another program wrote and rhwp does not read: a
 * history the package's manifest names, notes it does not name, and a piece
 * of data it names, which rhwp keeps under a name of its own.
 */
export function koreanHwpxWithOtherParts(): Buffer {
  const members: ZipMember[] = [];
  rewriteArchive(koreanHwpx(), (member) => {
    if (member.name !== 'Contents/content.hpf') {
      members.push(member);
      return member;
    }
    const listed = member.data
      .toString('utf8')
      .replace('</opf:manifest>', `${OTHER_ITEMS}</opf:manifest>`);
    if (!listed.includes(OTHER_ITEMS)) throw new Error('the package lists its parts otherwise');
    members.push({ ...member, data: Buffer.from(listed) });
    return member;
  });
  return zip([...members, ...OTHER_PARTS]);
}

export function koreanHwp(): Buffer {
  return Buffer.from(koreanDocument().exportHwp());
}

/** An HWP document with `streams` more. */
export function withStreams(document: Buffer, streams: readonly CompoundStream[]): Buffer {
  return compoundFile([...compoundStreams(document), ...streams]);
}

/** A stream another program wrote, which rhwp does not read and keeps as it is. */
export const OTHER_STREAM: CompoundStream = {
  path: 'Custom/Notes',
  data: Buffer.from('기록 notes kept by another program'),
};

/** The text of the distribution sample's protected body, which its open body does not have. */
export const DISTRIBUTED_TEXT = '배포본문';

/** The record that opens a section of `ViewText` and hides its key (`HWPTAG_DISTRIBUTE_DOC_DATA`). */
const DISTRIBUTE_DOC_DATA = 0x1c;
/** The property of `FileHeader` that says a document is a distribution document. */
const DISTRIBUTION = 1 << 2;

/** The random numbers of Microsoft's C library from `seed`, which the record's bytes are hidden with. */
function msvcRandom(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 214013) + 2531011) >>> 0;
    return (state >>> 16) & 0x7fff;
  };
}

/**
 * A section of `ViewText` as Hangul writes one, as rhwp reads it: a 256-byte record whose
 * bytes past the first four are hidden by a stream of numbers seeded with those four, the
 * key of AES-128 among them where the first byte says; then the section's bytes, encrypted
 * with that key (ECB), whole blocks.
 */
function viewTextSection(section: Buffer): Buffer {
  const plain = Buffer.alloc(256);
  plain.writeUInt32LE(0x2b7e1516, 0);
  const key = Buffer.from('fvoci-test-key16');
  key.copy(plain, 4 + ((plain[0] ?? 0) & 0x0f));
  const random = msvcRandom(plain.readUInt32LE(0));
  const hidden = Buffer.from(plain);
  for (let at = 0, left = 0, mask = 0; at < hidden.length; at += 1, left -= 1) {
    if (left === 0) {
      mask = random() & 0xff;
      left = (random() & 0x0f) + 1;
    }
    if (at >= 4) hidden[at] = (plain[at] ?? 0) ^ mask;
  }
  const blocks = Buffer.alloc(Math.ceil(section.length / 16) * 16);
  section.copy(blocks);
  const cipher = createCipheriv('aes-128-ecb', key, null).setAutoPadding(false);
  const head = Buffer.alloc(4);
  head.writeUInt32LE(DISTRIBUTE_DOC_DATA | (hidden.length << 20), 0);
  return Buffer.concat([head, hidden, cipher.update(blocks), cipher.final()]);
}

/**
 * The Korean HWP as a distribution document (배포용 문서), which Hangul keeps from being
 * copied and printed: the property that says so in `FileHeader`, and the body in
 * `ViewText/Section0`, encrypted. The open body, `BodyText/Section0`, is the sample's
 * own; the protected one has `DISTRIBUTED_TEXT` too, so what rhwp shows tells which it read.
 */
export function distributionHwp(): Buffer {
  const streams = compoundStreams(koreanHwp()).map((stream) => {
    if (stream.path !== 'FileHeader') return stream;
    const data = Buffer.from(stream.data);
    data.writeUInt32LE(data.readUInt32LE(36) | DISTRIBUTION, 36);
    return { ...stream, data };
  });
  return compoundFile([...streams, protectedBody()]);
}

/**
 * The Korean HWP with a protected body left in `ViewText` although it is no distribution
 * document, as a copy of one can keep it once the protection is off: rhwp reads the open
 * body and does not keep `ViewText`, which its report does not say.
 */
export function withViewTextLeft(): Buffer {
  return withStreams(koreanHwp(), [protectedBody()]);
}

/** The sample's body with `DISTRIBUTED_TEXT`, as `ViewText/Section0`. */
function protectedBody(): CompoundStream {
  const protectedBody = koreanDocument();
  done(protectedBody.insertText(0, 0, 0, DISTRIBUTED_TEXT));
  const section = compoundStreams(Buffer.from(protectedBody.exportHwp())).find(
    (stream) => stream.path === 'BodyText/Section0',
  );
  if (!section) throw new Error('the sample has no section');
  return { path: 'ViewText/Section0', data: viewTextSection(section.data) };
}

/**
 * The Korean HWPX with 30,000 characters in place of the body, on paper four
 * times as wide and as high, which holds them on one page: rhwp draws each
 * apart, and its first page comes to about 23,000 elements (past the frame's
 * ceiling of 20,000); the second is as it was. (On the sample's own paper
 * rhwp 0.9.0 breaks the paragraph over 17 pages of about 1,500 elements.)
 */
export function denseHwpx(): Buffer {
  const words = '가나다라 마바사아 자차카타 파하 ';
  const body = words.repeat(Math.ceil(30_000 / words.length)).slice(0, 30_000);
  const paper = /(<hp:pagePr[^>]*?width=")(\d+)("[^>]*?height=")(\d+)/;
  return rewriteArchive(koreanHwpx(), (member) => {
    if (member.name !== 'Contents/section0.xml') return member;
    const text = member.data.toString('utf8');
    if (!text.includes(HWP_TEXT.body) || !paper.test(text)) {
      throw new Error('the sample has no body or no paper to change');
    }
    const dense = text
      .replace(HWP_TEXT.body, body)
      .replace(
        paper,
        (_, a: string, width: string, b: string, height: string) =>
          `${a}${Number(width) * 4}${b}${Number(height) * 4}`,
      );
    return { ...member, data: Buffer.from(dense) };
  });
}

/**
 * The Korean HWPX with its body's paragraph after it `count` times more: a
 * long document that rhwp takes seconds to lay out as it opens it (60,000
 * make 1,465 pages, 3.3 s in Node on the machine of 2026-11; the archive is
 * 113 KB). Many more go past what the worker takes on.
 */
export function longHwpx(count = 60_000): Buffer {
  return rewriteArchive(koreanHwpx(), (member) => {
    if (member.name !== 'Contents/section0.xml') return member;
    const text = member.data.toString('utf8');
    const at = text.indexOf(HWP_TEXT.body);
    const start = text.lastIndexOf('<hp:p ', at);
    const end = text.indexOf('</hp:p>', at) + '</hp:p>'.length;
    if (at < 0 || start < 0 || end < at) throw new Error('the sample has no body paragraph');
    const paragraph = text.slice(start, end);
    return {
      ...member,
      data: Buffer.from(text.slice(0, end) + paragraph.repeat(count) + text.slice(end)),
    };
  });
}

/**
 * An HWPX whose header says it has `count` sections. rhwp (from 0.9.0) reads
 * only as many of the spine's sections as the header says, when it says fewer
 * than the spine names; the sample's says one.
 */
export function declaringSections(archive: Buffer, count: number): Buffer {
  const declared = /secCnt="\d+"/;
  return rewriteArchive(archive, (member) => {
    if (member.name !== 'Contents/header.xml') return member;
    const text = member.data.toString('utf8');
    if (!declared.test(text)) throw new Error('the header declares no count of sections');
    return { ...member, data: Buffer.from(text.replace(declared, `secCnt="${count}"`)) };
  });
}

export function lockedHwp(): Buffer {
  return Buffer.from(koreanDocument().exportHwpWithPassword(HWP_PASSWORD));
}

export function lockedHwpx(): Buffer {
  return Buffer.from(koreanDocument().exportHwpxWithPassword(HWP_PASSWORD));
}

/** A stream of a compound file, and the lies the file can tell about it. */
export interface CompoundStream {
  /** Its path, with "/" between storages: `BodyText/Section0`. */
  path: string;
  data: Buffer;
  /** The size its directory entry gives, when not its own. */
  said?: number;
  /** Its entry points at the first sector of this stream's chain instead of its own. */
  sharesWith?: string;
}

export interface CompoundOptions {
  /** Makes the chain of the stream at this path come back to its first sector. */
  cycle?: string;
  /** 3: sectors of 512 bytes, as HWP files have; 4: of 4096. */
  version?: 3 | 4;
}

/** The index of the entry at `path` in the directory of `file`. */
function entryIndex(file: CompoundFile, path: string): number {
  let at: CompoundEntry | undefined = file.entries[0];
  for (const name of path.split('/')) {
    at = at && childrenOf(file, at).find((entry) => entry.name === name);
  }
  const index = at ? file.entries.indexOf(at) : -1;
  if (index < 0) throw new Error(`no entry ${path}`);
  return index;
}

/** Where entry `index` of the directory lies in the file. */
function entryAt(file: CompoundFile, index: number): number {
  const perSector = file.sectorSize / 128;
  const sector = file.directorySectors[Math.floor(index / perSector)] ?? 0;
  return (sector + 1) * file.sectorSize + (index % perSector) * 128;
}

/**
 * A compound file of the streams, as the frame writes one
 * (src/viewer-frame/compound-writer.ts), then made to tell the lies asked of
 * it: a size of a stream's own, a chain it shares, a chain that comes back.
 */
export function compoundFile(
  streams: readonly CompoundStream[],
  { cycle, version = 3 }: CompoundOptions = {},
): Buffer {
  const parts = streams.map(({ path, data }) => ({ names: path.split('/'), data }));
  const out = Buffer.from(writeCompound(parts, version));
  const file = readCompound(new Uint8Array(out.buffer, out.byteOffset, out.byteLength));
  if (!file) throw new Error('the writer wrote no compound file');
  for (const { path, said, sharesWith } of streams) {
    const at = entryAt(file, entryIndex(file, path));
    if (said !== undefined) out.writeUInt32LE(said, at + 120);
    if (sharesWith !== undefined) {
      const shared = file.entries[entryIndex(file, sharesWith)];
      out.writeUInt32LE(shared?.start ?? 0, at + 116);
    }
  }
  if (cycle !== undefined) {
    const entry = file.entries[entryIndex(file, cycle)];
    const sectors = entry ? chain(file.fat, entry.start) : undefined;
    const last = sectors?.at(-1);
    if (!entry || last === undefined) throw new Error(`no chain for ${cycle}`);
    const perSector = file.sectorSize / 4;
    const fatSector = file.fatSectors[Math.floor(last / perSector)] ?? 0;
    out.writeUInt32LE(entry.start, (fatSector + 1) * file.sectorSize + (last % perSector) * 4);
  }
  return out;
}

/** Every stream of a compound file with its path, read with the viewer's own reader. */
export function compoundStreams(data: Buffer): CompoundStream[] {
  const file = readCompound(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
  const root = file?.entries[0];
  if (!file || !root) throw new Error('not a compound file');
  const walk = (storage: CompoundEntry, prefix: string): CompoundStream[] =>
    childrenOf(file, storage).flatMap((entry) => {
      const path = `${prefix}${entry.name}`;
      if (entry.type === COMPOUND_TYPES.STORAGE) return walk(entry, `${path}/`);
      const read = readStream(file, entry, entry.size);
      if (!read) throw new Error(`the stream ${path} does not read`);
      return [{ path, data: Buffer.from(read) }];
    });
  return walk(root, '');
}

/** A `FileHeader` stream of an HWP document: its signature, version 5.1, and `properties`. */
export function hwpFileHeader(properties = 0): Buffer {
  const header = Buffer.alloc(256);
  header.write('HWP Document File', 'ascii');
  header.writeUInt32LE(0x05010000, 32);
  header.writeUInt32LE(properties, 36);
  return header;
}
