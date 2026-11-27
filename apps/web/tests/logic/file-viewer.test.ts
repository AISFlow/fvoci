import fc from 'fast-check';
import { describe, expect, it } from 'vitest';

import {
  examine,
  MARKUP_LIMIT_BYTES,
  UNPACKED_LIMIT_BYTES,
  type FormatId,
} from '$lib/file-viewer/formats.ts';
import {
  LABEL_LIMIT,
  PAGE_LIMIT,
  readFromFrame,
  SHEET_LIMIT,
  SHEET_NAME_LENGTH,
  readToFrame,
  VIEWER_KEYS,
  ZOOM_MAX,
  ZOOM_MIN,
} from '$lib/file-viewer/protocol.ts';
import { zipEntries } from '$lib/file-viewer/zip-directory.ts';
import { UnreadableFile } from '../../src/viewer-frame/renderer.ts';
import { MarkupSniffer, repack } from '../../src/viewer-frame/zip-guard.ts';

import { blankPaddedDocx, koreanDocx, koreanPdf, png, zip } from '../support/viewer-files.ts';
import { koreanXlsx } from '../support/viewer-xlsx.ts';

const bytes = (buffer: Buffer): Uint8Array =>
  new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
const text = (value: string): Uint8Array => new TextEncoder().encode(value);
const zipMemberNames = (data: Uint8Array): string[] | undefined =>
  zipEntries(data)?.map((entry) => entry.name);
/** The kind the viewer draws the bytes as, or undefined when it does not draw them. */
const sniffFormat = (data: Uint8Array): FormatId | undefined => {
  const examined = examine(data);
  return examined.kind === 'format' ? examined.format : undefined;
};

describe('a file is told apart by its first bytes, not by its name', () => {
  it('a PDF', () => {
    expect(sniffFormat(bytes(koreanPdf(2)))).toBe('pdf');
  });

  it('a PDF with something before its header, within the first 1024 bytes', () => {
    expect(sniffFormat(text(`${' '.repeat(1000)}%PDF-1.4\n`))).toBe('pdf');
    expect(sniffFormat(text(`${' '.repeat(1100)}%PDF-1.4\n`))).toBeUndefined();
  });

  it('a Word document: a ZIP archive with word/document.xml', () => {
    expect(sniffFormat(bytes(koreanDocx()))).toBe('docx');
  });

  it('a workbook: a ZIP archive with xl/workbook.xml', () => {
    expect(sniffFormat(bytes(koreanXlsx()))).toBe('xlsx');
  });

  it('another ZIP archive, a picture, text and nothing are none of them', () => {
    // An OpenDocument spreadsheet, and parts named nearly as the Office ones are.
    const archive = zip([
      { name: 'mimetype', data: Buffer.from('application/vnd.oasis.opendocument.spreadsheet') },
      { name: 'content.xml', data: Buffer.from('<office:document-content/>') },
      { name: 'xl/workbook.xml.bak', data: Buffer.from('x') },
      { name: 'word/document.xml.bak', data: Buffer.from('x') },
    ]);
    expect(sniffFormat(bytes(archive))).toBeUndefined();
    expect(sniffFormat(bytes(png(4, 4, [0, 0, 0])))).toBeUndefined();
    expect(sniffFormat(text('%PD'))).toBeUndefined();
    expect(sniffFormat(text('plain text, named report.pdf'))).toBeUndefined();
    expect(sniffFormat(new Uint8Array())).toBeUndefined();
  });
});

describe('an archive is held to what its directory says it unpacks to', () => {
  const docx = (members: { name: string; said: number }[]): Uint8Array =>
    bytes(
      zip([
        { name: 'word/document.xml', data: Buffer.from('<w/>') },
        ...members.map(({ name, said }) => ({ name, data: Buffer.from('x'), said })),
      ]),
    );

  it('all its parts together, up to 200 MB', () => {
    // The main part says its own 4 bytes; the picture says the rest.
    const at = docx([{ name: 'word/media/a.png', said: UNPACKED_LIMIT_BYTES - 4 }]);
    const past = docx([{ name: 'word/media/a.png', said: UNPACKED_LIMIT_BYTES - 3 }]);
    expect(examine(at)).toEqual({ kind: 'format', format: 'docx' });
    expect(examine(past)).toEqual({ kind: 'too-large', format: 'docx' });
  });

  it('its XML parts (.xml, .rels) together, up to 30 MB', () => {
    const at = docx([{ name: 'word/styles.xml', said: MARKUP_LIMIT_BYTES - 4 }]);
    const past = docx([{ name: '_rels/.rels', said: MARKUP_LIMIT_BYTES - 3 }]);
    expect(examine(at)).toEqual({ kind: 'format', format: 'docx' });
    expect(examine(past)).toEqual({ kind: 'too-large', format: 'docx' });
    // The same size in a picture is within the limits.
    expect(examine(docx([{ name: 'word/media/a.png', said: MARKUP_LIMIT_BYTES }]))).toEqual({
      kind: 'format',
      format: 'docx',
    });
  });
});

describe('in the frame, an archive is counted and handed on anew', () => {
  const buffer = (data: Buffer): ArrayBuffer => Uint8Array.from(data).buffer;
  const parts = (data: ArrayBuffer): string[] | undefined =>
    zipEntries(new Uint8Array(data))?.map((entry) => `${entry.name}:${entry.size}`);
  const unreadable = (data: Buffer): Promise<void> =>
    expect(repack(buffer(data))).rejects.toBeInstanceOf(UnreadableFile);

  it('an honest archive keeps its parts, deflated or stored', async () => {
    const honest = zip([
      { name: 'word/document.xml', data: Buffer.from('<w/>') },
      { name: 'word/media/a.png', data: png(8, 8, [1, 2, 3]), stored: true },
    ]);
    const again = await repack(buffer(honest));
    expect(parts(again)).toEqual(parts(buffer(honest)));
    expect(parts(await repack(buffer(koreanDocx())))).toEqual(parts(buffer(koreanDocx())));
  });

  it('counting gives the thread back while it reads a part, stored or deflated', async () => {
    // Blanks are read one by one for the first character that is not one: tens of milliseconds.
    const blanks = Buffer.alloc(20_000_000, 0x20);
    for (const stored of [true, false]) {
      let ran = false;
      setTimeout(() => {
        ran = true;
      }, 0);
      const counted = await repack(
        buffer(zip([{ name: 'ppt/media/blank.bin', data: blanks, stored }])),
      );
      expect(parts(counted)).toEqual(['ppt/media/blank.bin:20000000']);
      expect(ran, stored ? 'stored' : 'deflated').toBe(true);
    }
  });

  it('a part that holds more than its directory says stops the file', async () => {
    const said = 1000;
    const data = Buffer.alloc(1_000_000);
    await unreadable(zip([{ name: 'word/media/a.png', data, said }]));
    await unreadable(zip([{ name: 'word/media/a.png', data, said, stored: true }]));
    // So do bytes that are not deflated data.
    const broken = koreanDocx();
    broken.fill(0xff, 50, 70);
    await unreadable(broken);
  });

  it('what another reader would find besides what was counted is not handed on', async () => {
    const hidden = { name: 'word/document.xml', data: Buffer.alloc(1_000_000, 0x20) };
    // The end says one entry; a reader that goes on finds a second of the same name.
    const small = { name: 'word/document.xml', data: Buffer.from('<w/>') };
    const counted = await repack(buffer(zip([small, hidden], 1)));
    expect(parts(counted)).toEqual(['word/document.xml:4']);
    expect(counted.byteLength).toBeLessThan(200);
    // A local header that names another part: the new archive names it as the directory does.
    const renamed = zip([{ ...hidden, name: 'word/media/a.png', localName: 'word/document.xml' }]);
    const named = new Uint8Array(await repack(buffer(renamed)));
    expect(new TextDecoder().decode(named.subarray(30, 46))).toBe('word/media/a.png');
    // A name that is not UTF-8 keeps its bytes, rather than growing into U+FFFD.
    const raw = zip([{ name: 'x', data: Buffer.from('a') }]);
    raw.fill(0xff, raw.length - 23, raw.length - 22);
    const kept = new Uint8Array(await repack(buffer(raw)));
    expect(kept[30]).toBe(0xff);
    expect(kept.byteLength).toBe(raw.byteLength);
  });

  it('a name with a backslash stops the file: docx-preview would find it under many', async () => {
    await unreadable(zip([{ name: 'word\\header1.xml', data: Buffer.from('<w/>') }]));
  });

  it('parts that share packed bytes stop the file', async () => {
    const archive = zip([
      { name: 'a.xml', data: Buffer.from('<a/>') },
      { name: 'b.xml', data: Buffer.from('<b/>') },
    ]);
    // The second entry of the directory points at the first part.
    const directory = archive.readUInt32LE(archive.length - 6);
    const second = directory + 46 + 'a.xml'.length;
    archive.writeUInt32LE(0, second + 42);
    await unreadable(archive);
  });

  it('what the directory says is held to the limits here too', async () => {
    await unreadable(
      zip([{ name: 'word/media/a.png', data: Buffer.from('x'), said: UNPACKED_LIMIT_BYTES + 1 }]),
    );
  });

  it('its XML is told by what it holds, not by its name, and held to 30 MB', async () => {
    // The main part, named .bin, after blanks: up to the limit with the other XML, and one byte past.
    // Relationships count twice (docx-preview may parse them twice as often as a part), so
    // most of the limit is theirs here, in half as many bytes to unpack: reading 30 MB of
    // blanks one by one, twice, is not what is shown.
    const lead = 100_000;
    const base =
      zipEntries(bytes(blankPaddedDocx(lead)))?.reduce(
        (sum, entry) => sum + (entry.name.endsWith('.rels') ? 2 : 1) * entry.size,
        0,
      ) ?? 0;
    const padding = Math.floor((MARKUP_LIMIT_BYTES - base) / 2);
    const blanks = lead + (MARKUP_LIMIT_BYTES - base - 2 * padding);
    expect(parts(await repack(buffer(blankPaddedDocx(blanks, padding))))).toHaveLength(3);
    await unreadable(blankPaddedDocx(blanks + 1, padding));
  });

  it('XML is a first character "<" past any blanks, read as JSZip reads text', () => {
    const sniff = (...chunks: number[][]): boolean | undefined => {
      const sniffer = new MarkupSniffer();
      for (const chunk of chunks) sniffer.feed(Uint8Array.from(chunk));
      return sniffer.verdict;
    };
    const blanks = Array.from({ length: 5000 }, () => 0x20);
    expect(sniff([0x3c, 0x3f])).toBe(true);
    expect(sniff([0xef, 0xbb], [0xbf, 0x0a, 0x3c])).toBe(true);
    expect(sniff([0x00, 0x3c])).toBe(true);
    expect(sniff(blanks, blanks, [0x3c])).toBe(true);
    // Overlong forms, which JSZip's decoder in a browser reads as "<" and as a blank.
    expect(sniff([0xc0, 0xbc])).toBe(true);
    expect(sniff([0xc0], [0x3c])).toBe(true);
    expect(sniff([0xe0, 0x80, 0xbc])).toBe(true);
    expect(sniff([0xc0, 0xa0, 0x3c])).toBe(true);
    // A JPEG, a PNG, a UTF-16 mark (garbage to that decoder), text: none.
    expect(sniff([0xff, 0xd8, 0xff, 0xe0])).toBe(false);
    expect(sniff([0x89, 0x50, 0x4e, 0x47])).toBe(false);
    expect(sniff([0xff, 0xfe, 0x3c, 0x00])).toBe(false);
    expect(sniff([0xfe, 0xff, 0x00, 0x3c])).toBe(false);
    expect(sniff([0x20, 0xbb, 0x3c])).toBe(false);
    expect(sniff(blanks, [0x41, 0x3c])).toBe(false);
    expect(sniff(blanks)).toBeUndefined();
  });
});

describe('the names of a ZIP archive', () => {
  it('are read from its directory, in order', () => {
    const archive = zip([
      { name: '[Content_Types].xml', data: Buffer.from('a') },
      { name: '한글/이름.xml', data: Buffer.from('b') },
    ]);
    expect(zipMemberNames(bytes(archive))).toEqual(['[Content_Types].xml', '한글/이름.xml']);
  });

  it('are found after a comment at the end of the archive', () => {
    const archive = zip([{ name: 'word/document.xml', data: Buffer.from('<w/>') }]);
    const commented = Buffer.concat([archive, Buffer.from('a comment')]);
    commented.writeUInt16LE(9, archive.length - 2);
    expect(zipMemberNames(bytes(commented))).toEqual(['word/document.xml']);
  });

  it('are not read from bytes that only look like an archive', () => {
    const archive = zip([{ name: 'word/document.xml', data: Buffer.from('<w/>') }]);
    // The directory said to lie past the end of the file.
    const broken = Buffer.from(archive);
    broken.writeUInt32LE(archive.length, archive.length - 6);
    expect(zipMemberNames(bytes(broken))).toBeUndefined();
    expect(zipMemberNames(bytes(archive.subarray(0, archive.length - 30)))).toBeUndefined();
  });

  it('reading never throws, whatever the bytes', () => {
    const archive = zip([{ name: 'word/document.xml', data: Buffer.from('<w/>') }]);
    fc.assert(
      fc.property(fc.uint8Array({ maxLength: 200 }), fc.nat(archive.length - 1), (noise, at) => {
        const mixed = Uint8Array.from(archive);
        mixed.set(noise.subarray(0, archive.length - at), at);
        expect(() => sniffFormat(mixed)).not.toThrow();
        expect(() => zipMemberNames(noise)).not.toThrow();
      }),
    );
  });
});

describe('a message from the frame is taken only in one of its shapes', () => {
  it('each kind', () => {
    expect(readFromFrame({ kind: 'ready' })).toEqual({ kind: 'ready' });
    expect(readFromFrame({ kind: 'shown', pages: 3 })).toEqual({ kind: 'shown', pages: 3 });
    expect(readFromFrame({ kind: 'page', page: 2 })).toEqual({ kind: 'page', page: 2 });
    expect(readFromFrame({ kind: 'failed', reason: 'locked' })).toEqual({
      kind: 'failed',
      reason: 'locked',
    });
    for (const key of VIEWER_KEYS) {
      expect(readFromFrame({ kind: 'key', key })).toEqual({ kind: 'key', key });
    }
    expect(readFromFrame({ kind: 'download' })).toEqual({ kind: 'download' });
    const sheets = [
      { name: '매출', drawings: false },
      { name: '일정', drawings: true },
    ];
    expect(readFromFrame({ kind: 'sheets', sheets })).toEqual({ kind: 'sheets', sheets });
  });

  it("a workbook's sheets: one to a thousand, each named, its name at most 100 long", () => {
    const sheet = (name: string, drawings: unknown = false) => ({ name, drawings });
    const most = Array.from({ length: SHEET_LIMIT }, (_, index) => sheet(`Sheet${index + 1}`));
    expect(readFromFrame({ kind: 'sheets', sheets: most })).toBeDefined();
    expect(
      readFromFrame({ kind: 'sheets', sheets: [sheet('가'.repeat(SHEET_NAME_LENGTH))] }),
    ).toBeDefined();
    for (const sheets of [
      [],
      [...most, sheet('one more')],
      [sheet('')],
      [sheet('가'.repeat(SHEET_NAME_LENGTH + 1))],
      [sheet('a', 'yes')],
      [{ name: 'a', drawings: false, hidden: true }],
      [{ name: 'a' }],
      ['a'],
      'a',
    ]) {
      expect(readFromFrame({ kind: 'sheets', sheets }), JSON.stringify(sheets).slice(0, 80)).toBe(
        undefined,
      );
    }
    expect(readFromFrame({ kind: 'sheets', sheets: [sheet('a')], pages: 1 })).toBeUndefined();
  });

  it("a deck's hidden slides: numbers of pages, at least one, in order, each once", () => {
    expect(readFromFrame({ kind: 'hidden', pages: [1, 4, 52] })).toEqual({
      kind: 'hidden',
      pages: [1, 4, 52],
    });
    const most = Array.from({ length: PAGE_LIMIT }, (_, index) => index + 1);
    expect(readFromFrame({ kind: 'hidden', pages: most })).toBeDefined();
    for (const pages of [
      [],
      [0],
      [2, 1],
      [3, 3],
      [1.5],
      ['2'],
      [PAGE_LIMIT + 1],
      '1',
      [...most, 1],
    ]) {
      expect(readFromFrame({ kind: 'hidden', pages }), JSON.stringify(pages).slice(0, 80)).toBe(
        undefined,
      );
    }
    expect(readFromFrame({ kind: 'hidden', pages: [1], shown: true })).toBeUndefined();
  });

  it('not with a field more, a field less, or a value out of range', () => {
    for (const data of [
      undefined,
      null,
      'ready',
      ['ready'],
      {},
      { kind: 'ready', extra: 1 },
      { kind: 'shown' },
      { kind: 'shown', pages: 0 },
      { kind: 'shown', pages: 1.5 },
      { kind: 'shown', pages: PAGE_LIMIT + 1 },
      { kind: 'shown', pages: '3' },
      { kind: 'page', page: -1 },
      { kind: 'failed', reason: 'because' },
      { kind: 'key', key: 'Enter' },
      { kind: 'key', key: 'a' },
      { kind: 'download', name: 'other.exe' },
      { kind: 'open', format: 'pdf', bytes: new ArrayBuffer(1), theme: 'light' },
      { kind: 'navigate', url: 'https://example.com/' },
    ]) {
      expect(readFromFrame(data), JSON.stringify(data)).toBeUndefined();
    }
  });

  it('not from an object that is not plain', () => {
    class Shaped {
      kind = 'ready';
    }
    expect(readFromFrame(new Shaped())).toBeUndefined();
    expect(readFromFrame(Object.assign(Object.create(null) as object, { kind: 'ready' }))).toBe(
      undefined,
    );
  });
});

describe('a message from the app, in the frame', () => {
  it('opens a file of a kind the viewer draws, with its bytes', () => {
    const buffer = new ArrayBuffer(4);
    const labels = {
      chart: '미리 보기에는 차트가 나오지 않습니다',
      pageTooLarge: '이 쪽은 너무 커서 보이지 않습니다.',
      download: '다운로드',
      hidden: '숨김',
    };
    const rest = { theme: 'dark', lang: 'ko', labels };
    const open = { kind: 'open', format: 'docx', bytes: buffer, engine: null, ...rest };
    expect(readToFrame(open)).toEqual(open);
    // A Hangul document comes with rhwp's engine, and no other kind does.
    const hangul = { ...open, format: 'hwpx', engine: new ArrayBuffer(8) };
    expect(readToFrame(hangul)).toEqual(hangul);
    expect(readToFrame({ ...hangul, format: 'hwp' })).toEqual({ ...hangul, format: 'hwp' });
    expect(readToFrame({ ...hangul, engine: null })).toBeUndefined();
    expect(readToFrame({ ...hangul, engine: new Uint8Array(8) })).toBeUndefined();
    expect(readToFrame({ ...open, engine: new ArrayBuffer(8) })).toBeUndefined();
    expect(readToFrame({ kind: 'open', format: 'docx', bytes: buffer, ...rest })).toBeUndefined();
    expect(readToFrame({ ...open, format: 'pptx' })).toEqual({ ...open, format: 'pptx' });
    expect(readToFrame({ ...open, lang: 'zh-Hans' })).toEqual({ ...open, lang: 'zh-Hans' });
    const long = 'x'.repeat(LABEL_LIMIT);
    const most = {
      ...open,
      labels: { chart: long, pageTooLarge: long, download: long, hidden: long },
    };
    expect(readToFrame(most)).toEqual(most);
    for (const wrong of [
      { ...open, format: 'html' },
      { ...open, bytes: new Uint8Array(4) },
      { ...open, lang: 'ko"><script>' },
      { ...open, lang: '' },
      { kind: 'open', format: 'docx', bytes: buffer, engine: null, theme: 'dark', lang: 'ko' },
      { ...open, labels: {} },
      { ...open, labels: { chart: labels.chart } },
      { ...open, labels: { ...labels, chart: '' } },
      { ...open, labels: { ...labels, download: '' } },
      { ...open, labels: { ...labels, download: 7 } },
      { ...open, labels: { ...labels, hidden: '' } },
      { ...open, labels: { chart: long, pageTooLarge: long, download: long } },
      { ...open, labels: { ...labels, chart: 'a'.repeat(LABEL_LIMIT + 1) } },
      { ...open, labels: { ...labels, pageTooLarge: `${long}x` } },
      { ...open, labels: { ...labels, page: 'Page' } },
      { ...open, labels: 'Charts' },
    ]) {
      expect(readToFrame(wrong), JSON.stringify(wrong)).toBeUndefined();
    }
  });

  it('zooms within its range and goes to a page', () => {
    expect(readToFrame({ kind: 'zoom', zoom: ZOOM_MIN })).toEqual({ kind: 'zoom', zoom: ZOOM_MIN });
    expect(readToFrame({ kind: 'zoom', zoom: ZOOM_MAX * 2 })).toBeUndefined();
    expect(readToFrame({ kind: 'zoom', zoom: Number.NaN })).toBeUndefined();
    expect(readToFrame({ kind: 'page', page: 4 })).toEqual({ kind: 'page', page: 4 });
    expect(readToFrame({ kind: 'theme', theme: 'dim' })).toBeUndefined();
  });

  it("shows a deck's hidden slides or leaves them out", () => {
    expect(readToFrame({ kind: 'hidden', shown: true })).toEqual({ kind: 'hidden', shown: true });
    for (const wrong of [
      { kind: 'hidden', shown: 'yes' },
      { kind: 'hidden', shown: false, page: 3 },
      { kind: 'hidden' },
    ]) {
      expect(readToFrame(wrong), JSON.stringify(wrong)).toBeUndefined();
    }
  });
});
