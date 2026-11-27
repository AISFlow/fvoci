/**
 * A document saved from the editor is the document: what rhwp writes, of the
 * bytes the editor was given, opens again to the same pages, text, tables and
 * pictures, when nothing was typed and when a line was.
 *
 * The editor is given the bytes as the viewer's check wrote them anew
 * (`HwpReader.check`), not the file's own, so that is where each case starts;
 * and what is saved is read back through the same check, as the viewer will
 * read the new version.
 *
 * With `FVOCI_HWP_SAMPLES` set to a folder, every document under it goes the
 * same way, and what became of each is written, in numbers, to
 * `FVOCI_HWP_ROUNDTRIP_REPORT`. The folder is read, never run.
 */
import { appendFileSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import { HwpDocument } from '@rhwp/core';
import { describe, expect, it } from 'vitest';

import { lossesOf } from '../../src/editor-frame/losses.ts';
import { examine } from '../../src/lib/file-viewer/formats.ts';
import { HwpReader } from '../../src/viewer-frame/hwp-document.ts';
import type { HwpFormat } from '../../src/viewer-frame/hwp-messages.ts';

import { contentOf, differences, lost, type Content } from '../support/hwp-content.ts';
import {
  compoundStreams,
  koreanHwp,
  koreanHwpx,
  rewriteArchive,
  startRhwp,
} from '../support/hwp-files.ts';
import type { ZipMember } from '../support/viewer-files.ts';

const FOLDER = process.env.FVOCI_HWP_SAMPLES;
const REPORT = process.env.FVOCI_HWP_ROUNDTRIP_REPORT;
/** "2/8": the second of eight parts of the samples, for runs side by side. */
const PART = /^(\d+)\/(\d+)$/.exec(process.env.FVOCI_HWP_ROUNDTRIP_PART ?? '');
/** Samples left out, by their paths from the folder, one a line: those a run does not come back from. */
const LEFT_OUT = new Set((process.env.FVOCI_HWP_ROUNDTRIP_LEAVE ?? '').split('\n').filter(Boolean));

/** The line typed into a document: no white space, and nothing a document is likely to say. */
const TYPED = 'FVOCI편집확인';

const SAMPLES = [
  ['hwpx', koreanHwpx],
  ['hwp', koreanHwp],
] as const;

const copyOf = (data: Uint8Array): ArrayBuffer => {
  const copy = new ArrayBuffer(data.byteLength);
  new Uint8Array(copy).set(data);
  return copy;
};

/** The bytes as the editor, and the viewer, are given them: checked, and written anew. */
async function checked(format: HwpFormat, data: Uint8Array): Promise<Uint8Array> {
  const answer = await new HwpReader().answer({ kind: 'check', format, bytes: copyOf(data) });
  if (answer.kind !== 'checked') throw new Error(`refused: ${JSON.stringify(answer)}`);
  return new Uint8Array(answer.bytes);
}

/** What rhwp writes of a document for saving, with its report, as the editor asks for it. */
function saved(document: HwpDocument, format: HwpFormat): { bytes: Uint8Array; losses: number } {
  const exported =
    format === 'hwp' ? document.exportHwpWithReport() : document.exportHwpxWithReport();
  try {
    const { count } = lossesOf(JSON.parse(exported.contentLoss()));
    return { bytes: exported.takeBytes(), losses: count };
  } finally {
    exported.free();
  }
}

/** What `use` makes of the document in the bytes, which is let go afterwards. */
function reading<T>(bytes: Uint8Array, use: (document: HwpDocument) => T): T {
  const document = new HwpDocument(bytes);
  try {
    return use(document);
  } finally {
    document.free();
  }
}

function type(document: HwpDocument): void {
  const answer = JSON.parse(document.insertText(0, 0, 0, TYPED)) as { ok?: boolean };
  if (!answer.ok) throw new Error('rhwp did not take the text');
}

/** The content with what was typed taken out again. */
const untyped = (content: Content): Content => ({
  ...content,
  text: content.text.replace(TYPED, ''),
  words: content.words.replace(TYPED, ''),
});

const members = (archive: Uint8Array): ZipMember[] => {
  const all: ZipMember[] = [];
  rewriteArchive(Buffer.from(archive), (member) => {
    all.push(member);
    return member;
  });
  return all;
};

describe('the bytes the editor is given, the file written anew by the check', () => {
  it('have every part of an HWPX, as it was', async () => {
    const original = koreanHwpx();
    const given = await checked('hwpx', original);
    expect(members(given)).toEqual(members(original));
    expect(members(given).map((member) => member.name)).toContain('BinData/image1.png');
  });

  it('have every stream of an HWP, as it was', async () => {
    const original = koreanHwp();
    const given = await checked('hwp', original);
    // The streams are written in another order; each is there, with its bytes.
    const streams = (file: Buffer): string[] =>
      compoundStreams(file)
        .map((stream) => `${stream.path} ${stream.data.toString('base64')}`)
        .sort();
    expect(streams(Buffer.from(given))).toEqual(streams(original));
    const paths = compoundStreams(original).map((stream) => stream.path);
    expect(paths).toContain('BodyText/Section0');
    expect(paths).toContain('BinData/BIN0001.png');
  });
});

describe.each(SAMPLES)('a document of %s saved by rhwp', (format, sample) => {
  it('unedited, is the document: its pages, text, table and picture', async () => {
    const given = await checked(format, sample());
    const before = reading(given, contentOf);
    expect(before.pages).toBe(2);
    expect(before.tables).toHaveLength(1);
    expect(before.pictures).toHaveLength(1);
    expect(before.pictures[0]).not.toContain('not-read');
    // The picture is among what the pages draw, read by its bytes.
    expect(before.drawn.flat()).toHaveLength(1);
    expect(before.drawn.flat()).not.toContain('not-read');
    expect(before.tables[0]).toMatch(/^2x3:6:/);

    const { bytes, losses } = reading(given, (document) => saved(document, format));
    expect(losses).toBe(0);
    // What is saved is of the format it was, and the viewer's check takes it.
    expect(examine(bytes)).toMatchObject({ kind: 'format', format });
    const after = reading(await checked(format, bytes), contentOf);
    expect(differences(before, after)).toEqual([]);
    expect(after).toEqual(before);
  });

  it('with a line typed into it, is the document with that line', async () => {
    const given = await checked(format, sample());
    const before = reading(given, contentOf);
    const { bytes, losses } = reading(given, (document) => {
      type(document);
      return saved(document, format);
    });
    expect(losses).toBe(0);
    expect(examine(bytes)).toMatchObject({ kind: 'format', format });
    const after = reading(await checked(format, bytes), contentOf);
    expect(after.text).toContain(TYPED);
    expect(after.text).not.toBe(before.text);
    expect(untyped(after)).toEqual(before);
    expect(after.words).toContain(TYPED);
    expect(lost(before, untyped(after))).toEqual([]);
    expect(lost(before, after)).toEqual(['text']);
  });

  it('saved twice over, is still the document', async () => {
    const given = await checked(format, sample());
    const before = reading(given, contentOf);
    const once = reading(given, (document) => saved(document, format)).bytes;
    const twice = reading(await checked(format, once), (document) => saved(document, format));
    expect(twice.losses).toBe(0);
    expect(reading(await checked(format, twice.bytes), contentOf)).toEqual(before);
  });
});

describe('the comparison of a document with what was saved', () => {
  const done = (answer: string): void => {
    if (!(JSON.parse(answer) as { ok?: boolean }).ok) throw new Error(answer);
  };
  const changed = (change: (document: HwpDocument) => void): string[] => {
    const original = koreanHwpx();
    const before = reading(original, contentOf);
    const after = reading(original, (document) => {
      change(document);
      // Through a save, as a loss would come about.
      return reading(saved(document, 'hwpx').bytes, contentOf);
    });
    // Found whatever the pages too, as in a document whose lines an edit moved.
    const whatever = lost(before, after);
    expect(whatever.length).toBeGreaterThan(0);
    expect(differences(before, after)).toEqual(expect.arrayContaining(whatever));
    return differences(before, after);
  };

  it('finds a picture that is gone', () => {
    expect(
      changed((document) => {
        done(document.deletePictureControl(0, 3, 0));
      }),
    ).toEqual(['pictures', 'controls']);
  });

  it('finds a table that is gone, with the text of its cells', () => {
    expect(
      changed((document) => {
        done(document.deleteTableControl(0, 2, 0));
      }),
    ).toEqual(['text', 'tables', 'controls']);
  });

  it('finds text that is gone, and a page', () => {
    expect(
      changed((document) => {
        done(document.deleteRange(0, 1, 0, 1, 5));
      }),
    ).toEqual(['text']);
    const pages = changed((document) => {
      const last = document.getParagraphCount(0) - 1;
      done(document.deleteRange(0, 3, 0, last, document.getParagraphLength(0, last)));
    });
    expect(pages).toContain('pages');
    expect(pages).toContain('text');
  });

  it('finds a picture whose bytes are other bytes', async () => {
    const original = koreanHwpx();
    const other = rewriteArchive(original, (member) =>
      member.name === 'BinData/image1.png'
        ? { ...member, data: Buffer.concat([member.data, Buffer.from([0])]) }
        : member,
    );
    const before = reading(await checked('hwpx', original), contentOf);
    const after = reading(await checked('hwpx', other), contentOf);
    expect(differences(before, after)).toEqual(['pictures']);
    // By the bytes the page draws alone, as a picture inside a table's cell is known.
    expect(differences({ ...before, pictures: [] }, { ...after, pictures: [] })).toEqual([
      'pictures',
    ]);
  });

  it('finds a table whose cells are merged otherwise', async () => {
    const before = reading(await checked('hwpx', koreanHwpx()), contentOf);
    const [table = ''] = before.tables;
    const merged = { ...before, tables: [table.replace(/:[0-9a-f]+$/, ':0000000000000000')] };
    expect(differences(before, merged)).toEqual(['tables']);
  });
});

describe('a save that loses something of the document', () => {
  /** The sample with its picture's part gone from the archive: rhwp has nothing to write for it. */
  const withoutPicture = (): Buffer =>
    rewriteArchive(koreanHwpx(), (member) =>
      member.name.startsWith('BinData/') ? undefined : member,
    );

  it('says so in its report, which the frame reads into the message for the app', async () => {
    const given = await checked('hwpx', withoutPicture());
    const report = reading(given, (document) => {
      const exported = document.exportHwpxWithReport();
      try {
        return JSON.parse(exported.contentLoss()) as unknown;
      } finally {
        exported.free();
      }
    });
    expect(lossesOf(report)).toEqual({
      count: 1,
      losses: [
        {
          code: 'binaryContentEmptied',
          subject: 'binaryData',
          reason: 'resourceReadFailedOrLimitExceeded',
          path: 'BinData/image1.png',
        },
      ],
    });
  });

  it('is no report, and so no save, when the report is not as rhwp 0.9.0 words it', () => {
    const loss = {
      code: 'controlOmitted',
      subject: 'control',
      reason: 'unsupportedByOutputFormat',
      path: 'section[0]/paragraph[3]',
    };
    const report = { schemaVersion: 1, outputFormat: 'hwp', count: 1, losses: [loss] };
    expect(lossesOf(report)).toEqual({ count: 1, losses: [loss] });
    for (const wrong of [
      undefined,
      null,
      'none',
      { ...report, schemaVersion: 2 },
      { ...report, count: 0 },
      { ...report, count: 2 },
      { ...report, losses: undefined },
      { ...report, losses: [{ ...loss, code: 'somethingNew' }] },
      { ...report, losses: [{ ...loss, subject: 'page' }] },
      { ...report, losses: [{ ...loss, reason: 'because' }] },
      { ...report, losses: [{ ...loss, path: 7 }] },
      { ...report, losses: [null] },
      // One past what the message lists is read as every other is.
      { ...report, count: 201, losses: [...Array<unknown>(200).fill(loss), { ...loss, code: '' }] },
    ]) {
      expect(() => lossesOf(wrong), JSON.stringify(wrong ?? null).slice(0, 80)).toThrow();
    }
    const many = { ...report, count: 250, losses: Array<unknown>(250).fill(loss) };
    expect(lossesOf(many).count).toBe(250);
    expect(lossesOf(many).losses).toHaveLength(200);
    const long = { ...report, losses: [{ ...loss, path: 'p'.repeat(1000) }] };
    expect(lossesOf(long).losses[0]?.path).toHaveLength(300);
  });
});

/** What became of one sample: numbers, and the names of what differs. */
interface Outcome {
  file: string;
  format?: HwpFormat;
  bytes: number;
  /** Why the document was not taken through: not one, refused by the check, or not opened by rhwp. */
  skipped?: string;
  pages?: number;
  tables?: number;
  pictures?: number;
  unedited?: Saved;
  edited?: Saved;
}

interface Saved {
  bytes?: number;
  losses?: number;
  /**
   * What of the saved file differs from the document: saved as it was read, anything (its
   * pages and what each draws too); saved with a line typed, what the document held whatever
   * the pages (the line moves what follows it), and the line itself. None when it is it.
   */
  differs?: string[];
  /** With a line typed, what else differs in the pages and what they draw: the lines moved. */
  moved?: string[];
  /** What failed, when the save or the reading back did. */
  failed?: string;
}

const said = (error: unknown): string =>
  (error instanceof Error ? `${error.name}: ${error.message}` : String(error)).slice(0, 120);

/** Whether a save differs from the document without rhwp having reported a loss. */
const summaryUnreported = (save?: Saved): boolean =>
  (save?.differs?.length ?? 0) > 0 && save?.losses === 0;

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

async function savedAndRead(
  given: Uint8Array,
  format: HwpFormat,
  before: Content,
  edit?: (document: HwpDocument) => void,
): Promise<Saved> {
  try {
    const { bytes, losses } = reading(given, (document) => {
      edit?.(document);
      return saved(document, format);
    });
    const examined = examine(bytes);
    if (examined.kind !== 'format' || examined.format !== format) {
      return { bytes: bytes.length, losses, failed: `examined as ${JSON.stringify(examined)}` };
    }
    const after = reading(await checked(format, bytes), contentOf);
    if (!edit) return { bytes: bytes.length, losses, differs: differences(before, after) };
    const differs = lost(before, untyped(after));
    if (!after.words.includes(TYPED)) differs.push('typed');
    const moved = differences(before, untyped(after)).filter((what) => !differs.includes(what));
    return { bytes: bytes.length, losses, differs, moved };
  } catch (error) {
    if (error instanceof WebAssembly.RuntimeError) throw error;
    return { failed: said(error) };
  }
}

async function roundTrip(file: string): Promise<Outcome> {
  const data = readFileSync(file);
  const outcome: Outcome = { file, bytes: data.length };
  const examined = examine(data);
  if (examined.kind !== 'format' || (examined.format !== 'hwp' && examined.format !== 'hwpx')) {
    return { ...outcome, skipped: `examined as ${examined.kind}` };
  }
  const format = examined.format;
  outcome.format = format;
  let given: Uint8Array;
  let before: Content;
  try {
    given = await checked(format, data);
    before = reading(given, contentOf);
  } catch (error) {
    if (error instanceof WebAssembly.RuntimeError) throw error;
    return { ...outcome, skipped: said(error) };
  }
  outcome.pages = before.pages;
  outcome.tables = before.tables.length;
  outcome.pictures = before.pictures.length;
  outcome.unedited = await savedAndRead(given, format, before);
  outcome.edited = await savedAndRead(given, format, before, type);
  return outcome;
}

describe.skipIf(!FOLDER)('real documents saved by rhwp', () => {
  it(
    'are read back by the viewer’s check, none differing unless rhwp said what it lost',
    async () => {
      const folder = FOLDER ?? '';
      expect(statSync(folder).isDirectory(), folder).toBe(true);
      startRhwp();
      const outcomes: Outcome[] = [];
      const [part, parts] = PART ? [Number(PART[1]), Number(PART[2])] : [1, 1];
      for (const [at, file] of samplesUnder(folder).entries()) {
        const name = path.relative(folder, file);
        if (at % parts !== part - 1) continue;
        if (LEFT_OUT.has(name)) {
          outcomes.push({ file: name, bytes: statSync(file).size, skipped: 'left out' });
          continue;
        }
        // Written before each: a run that does not come back says where it stayed.
        if (REPORT) appendFileSync(`${REPORT}.progress`, `${new Date().toISOString()} ${name}\n`);
        outcomes.push({ ...(await roundTrip(file)), file: name });
      }
      const through = outcomes.filter((one) => !one.skipped);
      const count = (pick: (one: Outcome) => boolean): number => through.filter(pick).length;
      const same = (save?: Saved): boolean => save?.differs?.length === 0 && !save.failed;
      const summary = {
        files: outcomes.length,
        skipped: outcomes.length - through.length,
        through: through.length,
        pages: through.reduce((sum, one) => sum + (one.pages ?? 0), 0),
        uneditedSame: count((one) => same(one.unedited)),
        uneditedDiffers: count((one) => (one.unedited?.differs?.length ?? 0) > 0),
        uneditedFailed: count((one) => one.unedited?.failed !== undefined),
        uneditedWithLosses: count((one) => (one.unedited?.losses ?? 0) > 0),
        editedSame: count((one) => same(one.edited)),
        editedDiffers: count((one) => (one.edited?.differs?.length ?? 0) > 0),
        editedFailed: count((one) => one.edited?.failed !== undefined),
        editedWithLosses: count((one) => (one.edited?.losses ?? 0) > 0),
        editedMoved: count((one) => (one.edited?.moved?.length ?? 0) > 0),
        // A difference rhwp's report did not tell of: the loss nobody is asked about.
        uneditedDiffersUnreported: count(
          (one) => (one.unedited?.differs?.length ?? 0) > 0 && one.unedited?.losses === 0,
        ),
        editedDiffersUnreported: count(
          (one) => (one.edited?.differs?.length ?? 0) > 0 && one.edited?.losses === 0,
        ),
      };
      if (REPORT) writeFileSync(REPORT, `${JSON.stringify({ summary, outcomes }, null, 1)}\n`);
      expect(through.length).toBeGreaterThan(0);
      // What a save loses is what the writer is asked about: a difference rhwp did not report,
      // or a save that fails, is nobody's question. The files are named in the report.
      expect(
        through
          .filter(
            (one) =>
              summaryUnreported(one.unedited) ||
              summaryUnreported(one.edited) ||
              one.unedited?.failed !== undefined ||
              one.edited?.failed !== undefined,
          )
          .map((one) => one.file),
      ).toEqual([]);
    },
    4 * 60 * 60 * 1000,
  );
});
