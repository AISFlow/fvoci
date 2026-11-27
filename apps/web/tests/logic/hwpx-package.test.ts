import { HwpDocument } from '@rhwp/core';
import { describe, expect, it } from 'vitest';

import { MARKUP_LIMIT_BYTES } from '$lib/file-viewer/formats.ts';
import { HwpReader } from '../../src/viewer-frame/hwp-document.ts';
import {
  checkHwpxPackage,
  listed,
  LOOKUP_LIMIT,
  masterPagesOf,
  sectionsOf,
} from '../../src/viewer-frame/hwpx-package.ts';
import { TooLargeFile, UnreadableFile } from '../../src/viewer-frame/renderer.ts';

import { declaringSections, koreanHwpx, rewriteArchive } from '../support/hwp-files.ts';
import { zip, type ZipMember } from '../support/viewer-files.ts';
import { rhwpWasmBytes } from '../../tools/build/rhwp-wasm.ts';

const buffer = (data: Buffer): ArrayBuffer => Uint8Array.from(data).buffer;

/** A package list of the items (id, address) and a spine of the ids, as Hancom writes one. */
function contentHpf(items: [string, string][], spine: string[]): string {
  const manifest = items
    .map(([id, href]) => `<opf:item id="${id}" href="${href}" media-type="application/xml"/>`)
    .join('');
  const refs = spine.map((id) => `<opf:itemref idref="${id}" linear="yes"/>`).join('');
  return `<?xml version="1.0" encoding="UTF-8"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:manifest>${manifest}</opf:manifest><opf:spine>${refs}</opf:spine></opf:package>`;
}

/** An HWPX archive: its mimetype, a package list, and the other parts. */
function hwpx(content: string, parts: ZipMember[]): Buffer {
  return zip([
    { name: 'mimetype', data: Buffer.from('application/hwp+zip'), stored: true },
    { name: 'Contents/content.hpf', data: Buffer.from(content) },
    ...parts,
  ]);
}

const xml = (bytes: number): Buffer =>
  Buffer.concat([Buffer.from('<hs:sec>'), Buffer.alloc(bytes, 0x20), Buffer.from('</hs:sec>')]);

describe("an HWPX package's list, read as rhwp reads it", () => {
  it('finds the items and the spine, whatever the prefix', () => {
    const list = listed(
      contentHpf(
        [
          ['header', 'Contents/header.xml'],
          ['s0', 'Contents/section0.xml'],
          ['mp', 'Contents/masterpage0.xml'],
        ],
        ['s0'],
      ),
    );
    expect(list.items.map((item) => item.href)).toEqual([
      'Contents/header.xml',
      'Contents/section0.xml',
      'Contents/masterpage0.xml',
    ]);
    expect(sectionsOf(list)).toEqual(['Contents/section0.xml']);
    expect(masterPagesOf(list)).toEqual(['Contents/masterpage0.xml']);
  });

  it('takes values as written, a quoted ">", and leaves comments out', () => {
    // rhwp does not read entities in a value: "s&#48;" names no item "s0".
    const list = listed(
      `<package><manifest><item id="s&#48;" href='Contents/section0.xml' title="a > b" media-type="application/xml"/>` +
        `<!-- <itemref idref="s0"/> --></manifest><spine><itemref note=">" idref="s&#48;"/></spine></package>`,
    );
    expect(list.items).toEqual([
      { id: 's&#48;', href: 'Contents/section0.xml', mediaType: 'application/xml' },
    ]);
    expect(list.spine).toEqual(['s&#48;']);
    expect(sectionsOf(list)).toEqual(['Contents/section0.xml']);
  });

  it('an element by its local name, after the last ":", whatever comes before', () => {
    const list = listed(
      `<package><가:item id="s0" href="Contents/section0.xml" media-type="application/xml"/>` +
        `<a:b:itemref idref="s0"/><x:itemref idref="s0"/><itemrefs idref="s0"/></package>`,
    );
    expect(list.items.map((item) => item.id)).toEqual(['s0']);
    expect(list.spine).toEqual(['s0', 's0']);
  });

  it('markup that does not end is unreadable, at once', () => {
    const unclosed = `<package>${'<item x'.repeat(40_000)}`;
    const started = performance.now();
    expect(() => listed(unclosed)).toThrow(UnreadableFile);
    expect(() => listed('<package><!-- open')).toThrow(UnreadableFile);
    expect(performance.now() - started).toBeLessThan(500);
  });

  it('an end tag of many blanks is unreadable, at once', () => {
    const started = performance.now();
    expect(() => listed(`<package><a></a${' '.repeat(100_000)}x></package>`)).toThrow(
      UnreadableFile,
    );
    expect(performance.now() - started).toBeLessThan(500);
  });

  it('without a spine, takes every XML part whose address says "section", in order', () => {
    const list = listed(
      contentHpf(
        [
          ['b', 'Contents/section1.xml'],
          ['a', 'Contents/section0.xml'],
        ],
        [],
      ),
    );
    expect(sectionsOf(list)).toEqual(['Contents/section0.xml', 'Contents/section1.xml']);
  });
});

describe("a list's spine is looked up without going through the items for each name", () => {
  /** A list of `count` items and a spine that names them from the last: the worst order for rhwp. */
  const long = (count: number): { items: [string, string][]; spine: string[] } => {
    const items = Array.from({ length: count }, (_, at): [string, string] => [
      `s${at}`,
      `Contents/section${at}.xml`,
    ]);
    return { items, spine: items.map(([id]) => id).reverse() };
  };

  it('an id two items have names the first, as rhwp finds it', () => {
    const list = listed(
      contentHpf(
        [
          ['s0', 'Contents/section0.xml'],
          ['s0', 'Contents/section9.xml'],
        ],
        ['s0'],
      ),
    );
    expect(sectionsOf(list)).toEqual(['Contents/section0.xml']);
  });

  it('a spine and a manifest at the limit are read in one pass; past it the file is too large', () => {
    const side = Math.sqrt(LOOKUP_LIMIT);
    const { items, spine } = long(side);
    const list = listed(contentHpf(items, spine));
    // Each item's id is read as the items are gone through once, not once for each name of the
    // spine (which took over two seconds at a fifth of this).
    let reads = 0;
    const counted = list.items.map((item) => ({
      ...item,
      get id() {
        reads += 1;
        return item.id;
      },
    }));
    const sections = sectionsOf({ items: counted, spine: list.spine });
    expect(reads).toBeLessThanOrEqual(2 * side);
    expect(sections).toHaveLength(side);
    expect(sections[0]).toBe(`Contents/section${side - 1}.xml`);

    const more = long(side + 1);
    expect(() => sectionsOf(listed(contentHpf(more.items, more.spine)))).toThrow(TooLargeFile);
  });

  it('the worker refuses a package whose list is past the limit', async () => {
    const { items, spine } = long(Math.sqrt(LOOKUP_LIMIT) + 1);
    const archive = hwpx(contentHpf(items, spine), []);
    await expect(checkHwpxPackage(buffer(archive))).rejects.toBeInstanceOf(TooLargeFile);
  });
});

describe('in the worker, what rhwp parses of an HWPX as XML is held to the limit', () => {
  it('the sample passes', async () => {
    await expect(checkHwpxPackage(buffer(koreanHwpx()))).resolves.toBeUndefined();
  });

  it('a tag of the list may have 1,000 attributes; one more stops the file', async () => {
    // The item's id, href and media-type, and as many more as make up the count, written so.
    const withAttributes = (count: number, more = (at: number) => `a${String(at)}=""`) => {
      const written = Array.from({ length: count - 3 }, (_, at) => more(at)).join(' ');
      const content = contentHpf([['s0', 'Contents/section0.xml']], ['s0']).replace(
        '<opf:item id=',
        `<opf:item ${written} id=`,
      );
      return buffer(hwpx(content, [{ name: 'Contents/section0.xml', data: xml(10) }]));
    };
    await expect(checkHwpxPackage(withAttributes(1000))).resolves.toBeUndefined();
    await expect(checkHwpxPackage(withAttributes(1001))).rejects.toBeInstanceOf(UnreadableFile);
    // Each name counts, also one given a value without quotes, given twice, or given no value.
    for (const more of [(at: number) => `a${String(at)}=1`, () => 'a=""', () => 'a']) {
      await expect(checkHwpxPackage(withAttributes(1000, more))).resolves.toBeUndefined();
      await expect(checkHwpxPackage(withAttributes(1001, more))).rejects.toBeInstanceOf(
        UnreadableFile,
      );
    }
  });

  it('a spine that names a section twice stops the file', async () => {
    const twice = hwpx(contentHpf([['s0', 'Contents/section0.xml']], ['s0', 's0']), [
      { name: 'Contents/section0.xml', data: xml(10) },
    ]);
    await expect(checkHwpxPackage(buffer(twice))).rejects.toBeInstanceOf(UnreadableFile);
  });

  it('a section that does not start with "<" counts as XML all the same', async () => {
    const hidden = Buffer.concat([Buffer.from('x'), xml(MARKUP_LIMIT_BYTES)]);
    const archive = hwpx(contentHpf([['s0', 'Contents/section0.dat']], ['s0']), [
      { name: 'Contents/section0.dat', data: hidden },
    ]);
    await expect(checkHwpxPackage(buffer(archive))).rejects.toBeInstanceOf(TooLargeFile);
    // So does a part rhwp would not parse; pictures under BinData/ do not.
    const picture = hwpx(contentHpf([['s0', 'Contents/section0.xml']], ['s0']), [
      { name: 'Contents/section0.xml', data: xml(10) },
      { name: 'BinData/image1.png', data: Buffer.alloc(MARKUP_LIMIT_BYTES + 1) },
    ]);
    await expect(checkHwpxPackage(buffer(picture))).resolves.toBeUndefined();
  });

  it('a part named with an entity in its address is the part of those very characters', async () => {
    // rhwp opens "BinData/section&amp;1.bin" as written: the part holds XML it parses.
    const name = 'BinData/section&amp;1.bin';
    const hidden = Buffer.concat([Buffer.from('x'), xml(MARKUP_LIMIT_BYTES)]);
    const archive = hwpx(contentHpf([['s1', name]], ['s1']), [{ name, data: hidden }]);
    await expect(checkHwpxPackage(buffer(archive))).rejects.toBeInstanceOf(TooLargeFile);
  });

  it('master pages count once for each section', async () => {
    const sections = (count: number): [string, string][] =>
      Array.from({ length: count }, (_, index) => [`s${index}`, `Contents/section${index}.xml`]);
    const archive = (count: number): Buffer =>
      hwpx(
        contentHpf(
          [...sections(count), ['mp', 'Contents/masterpage0.xml']],
          sections(count).map(([id]) => id),
        ),
        [
          ...sections(count).map(([, name]) => ({ name, data: xml(10) })),
          { name: 'Contents/masterpage0.xml', data: xml(1024 * 1024) },
        ],
      );
    await expect(checkHwpxPackage(buffer(archive(20)))).resolves.toBeUndefined();
    await expect(checkHwpxPackage(buffer(archive(40)))).rejects.toBeInstanceOf(TooLargeFile);
  });
});

/** rhwp's WebAssembly, as the build hands it to the frame. */
const WASM = rhwpWasmBytes().buffer;

describe("the worker's reader holds an HWPX to the limits before rhwp reads it", () => {
  const open = (data: Buffer) =>
    new HwpReader().answer({ kind: 'open', format: 'hwpx', bytes: buffer(data), wasm: WASM });
  /** The sample with its section rewritten by `change`. */
  const sample = (change: (member: ZipMember) => ZipMember): Buffer =>
    rewriteArchive(koreanHwpx(), (member) =>
      member.name === 'Contents/section0.xml' || member.name === 'Contents/content.hpf'
        ? change(member)
        : member,
    );

  it('opens the sample as it is, and as rewritten unchanged', async () => {
    expect((await open(sample((member) => member))).kind).toBe('opened');
  });

  it('stops the sample when its section unpacks to more than it says', async () => {
    const lying = sample((member) =>
      member.name === 'Contents/section0.xml' ? { ...member, said: 100 } : member,
    );
    expect(await open(lying)).toEqual({ kind: 'failed', reason: 'unreadable' });
  });

  it('stops the sample when its spine names its section twice', async () => {
    const twice = sample((member) => {
      if (member.name !== 'Contents/content.hpf') return member;
      const text = member.data.toString('utf8');
      const ref = /<opf:itemref idref="section0"[^>]*\/>/.exec(text)?.[0] ?? '';
      expect(ref).not.toBe('');
      return { ...member, data: Buffer.from(text.replace(ref, ref + ref)) };
    });
    expect(await open(twice)).toEqual({ kind: 'failed', reason: 'unreadable' });
  });
});

/** How many pages rhwp opens a file with, read directly; 'refused' when it does not. */
function rhwpPages(file: Buffer): number | 'refused' {
  try {
    const document = new HwpDocument(new Uint8Array(file));
    const pages = document.pageCount();
    document.free();
    return pages;
  } catch {
    return 'refused';
  }
}

describe('what the check reads of the spine is what rhwp reads', () => {
  const REF = '<opf:itemref idref="section0"/>';
  const QUOTED_REF = "<opf:itemref idref='section0'/>";
  /**
   * Markup put after the sample's own reference to its section, and how many
   * pages rhwp opens it with: 2 when it reads the section once, 4 when it
   * reads it twice, or none when it refuses the file. The sample's header is
   * made to say two sections: rhwp (from 0.9.0) reads no more of the spine's
   * than the header says, and the sample's own says one.
   */
  const cases: [string, string, 2 | 4 | 'refused'][] = [
    ['nothing', '', 2],
    ['a quote that is not after "="', `<x a'="'>${REF}</x>`, 4],
    [
      'a ">" inside a quoted value',
      `<x a="><opf:itemref idref='section0'/><y b="></y></x>`,
      'refused',
    ],
    ['an end tag that holds a quote', `<x></x a=">">${REF}`, 'refused'],
    ['a comment begun in a quoted entity', `<!DOCTYPE p [ <!ENTITY e '>  <!-- '> ]>${REF} -->`, 4],
    ['a reference in a quoted entity', `<!DOCTYPE p [ <!ENTITY e "${QUOTED_REF}"> ]>`, 2],
    ['a doctype in small letters', `<!doctype p '>'>${REF}`, 4],
    ['character data', `<![CDATA[${REF}]]>`, 2],
    ['an instruction', `<?note ${REF}?>`, 2],
    ['an instruction of nothing', `<??>${REF}`, 4],
    ['a comment with "--" inside', `<!-- a -- b -->${REF}`, 4],
    ['a name with no value first', '<opf:itemref x idref="section0"/>', 4],
    ['an unquoted value, then the name again', '<opf:itemref idref=section0 idref="section0"/>', 2],
    ['no blank between two attributes', '<opf:itemref a="x"idref="section0"/>', 4],
    ['blanks around "="', '<opf:itemref idref = "section0"/>', 4],
    ['a slash before the attributes', '<opf:itemref/ idref="section0">', 'refused'],
    ['a reference in the text', `&amp;${REF}`, 4],
    ['an "&" that is no reference', `& ${REF}`, 'refused'],
  ];

  /** The sample, its header saying `declared` sections, with `markup` after its reference. */
  const withMarkup = (markup: string, declared: number): Buffer =>
    rewriteArchive(declaringSections(koreanHwpx(), declared), (member) => {
      if (member.name !== 'Contents/content.hpf') return member;
      const text = member.data.toString('utf8');
      const ref = /<opf:itemref idref="section0"[^>]*\/>/.exec(text)?.[0] ?? '';
      expect(ref).not.toBe('');
      return { ...member, data: Buffer.from(text.replace(ref, ref + markup)) };
    });

  it('a header that says fewer sections than the spine names stops rhwp at that many; the check counts them all', async () => {
    // The spine names the section twice. rhwp reads it once where the header says one
    // section and twice where it says two, or none (0), or more than the spine has.
    for (const [declared, pages] of [
      [1, 2],
      [2, 4],
      [0, 4],
      [9, 4],
    ] as const) {
      const file = withMarkup(REF, declared);
      expect(rhwpPages(file), `secCnt ${declared}`).toBe(pages);
      // What the header says is the file's to say: the check goes by the spine.
      await expect(checkHwpxPackage(buffer(file))).rejects.toBeInstanceOf(UnreadableFile);
    }
  });

  it.each(cases)('%s', async (_, markup, pages) => {
    const file = withMarkup(markup, 2);
    expect(rhwpPages(file)).toBe(pages);
    const checked = await checkHwpxPackage(buffer(file)).then(
      () => 'passes',
      (error: unknown) => (error instanceof UnreadableFile ? 'unreadable' : String(error)),
    );
    expect(checked).toBe(pages === 2 ? 'passes' : 'unreadable');
  });
});

describe('the list the check reads is the one rhwp reads', () => {
  it('an archive that holds a part twice is refused: rhwp reads the last, in either order', async () => {
    const members: ZipMember[] = [];
    // Its header says two sections, so that rhwp reads the spine's two (see above).
    rewriteArchive(declaringSections(koreanHwpx(), 2), (member) => {
      members.push(member);
      return member;
    });
    const list = members.find((member) => member.name === 'Contents/content.hpf');
    if (!list) throw new Error('no list');
    const text = list.data.toString('utf8');
    const ref = /<opf:itemref idref="section0"[^>]*\/>/.exec(text)?.[0] ?? '';
    const twice = { ...list, data: Buffer.from(text.replace(ref, ref + ref)) };
    const orders: [ZipMember[], number][] = [
      [[list, twice], 4],
      [[twice, list], 2],
    ];
    for (const [lists, pages] of orders) {
      const file = zip(members.flatMap((member) => (member === list ? lists : [member])));
      expect(rhwpPages(file)).toBe(pages);
      await expect(checkHwpxPackage(buffer(file))).rejects.toBeInstanceOf(UnreadableFile);
    }
  });

  it('a list past the XML limit is too large, before it is read', async () => {
    // Read, this list would be unreadable: its markup does not end.
    const content = `${' '.repeat(MARKUP_LIMIT_BYTES)}<opf:package`;
    await expect(checkHwpxPackage(buffer(hwpx(content, [])))).rejects.toBeInstanceOf(TooLargeFile);
  });
});
