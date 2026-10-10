import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { crc32, deflateRawSync } from "node:zlib";
import { type Fault, cookieJarText, startFakeServer } from "./smoke-documents.fixture.ts";
import { cookieHeader, isPdf, officeHasText, parseCookieJar } from "./smoke-documents.ts";
import { xmlText } from "./xml-text.ts";
import { openZip, readZip, writeZip } from "./zip.ts";

const SCRIPT = join(import.meta.dir, "smoke-documents.ts");
const cleanups: Array<() => Promise<void>> = [];
afterEach(async () => {
  for (const cleanup of cleanups.splice(0)) await cleanup();
});

function setup(faults: Fault[] = [], docx?: Uint8Array) {
  const fake = startFakeServer(new Set(faults), docx);
  const dir = mkdtempSync(join(tmpdir(), "fvoci-smoke-documents-"));
  cleanups.push(async () => {
    await fake.server.stop(true);
    rmSync(dir, { recursive: true, force: true });
  });
  const jar = join(dir, "cookies");
  writeFileSync(jar, cookieJarText());
  const state = join(dir, "documents.json");
  const run = async (phase: string) => {
    const proc = Bun.spawn([process.execPath, SCRIPT, fake.base, "ws1", jar, state, phase], {
      stdout: "pipe",
      stderr: "pipe",
    });
    const [stdout, stderr, code] = await Promise.all([
      new Response(proc.stdout).text(),
      new Response(proc.stderr).text(),
      proc.exited,
    ]);
    return { code, stdout, stderr };
  };
  return { ...fake, jar, state, run };
}

describe("smoke-documents against a stand-in server", () => {
  test("create then restart pass and send the session only where required", async () => {
    const s = setup();
    const created = await s.run("create");
    expect(created.stderr).toBe("");
    expect(created.code).toBe(0);
    expect(created.stdout).toContain("public PDF and revocation: ok");
    const state = JSON.parse(readFileSync(s.state, "utf8")) as Record<string, unknown>;
    expect(state.document).toBe("doc1");
    expect(state.job).toBe("job1");
    const restarted = await s.run("restart");
    expect(restarted.code, restarted.stderr).toBe(0);
    expect(restarted.stdout).toContain("survive restart: ok");
    for (const request of s.requests) {
      expect(request.origin).toBe(s.base);
      expect(request.contentType).toBe("application/json");
    }
    const unauthenticated = s.requests.filter((r) => r.cookie === null).map((r) => r.path);
    expect(unauthenticated).toEqual([
      "/api/v1/workspaces/ws1/documents/doc1/md",
      "/api/v1/workspaces/ws1/documents/doc1/pdf",
      "/api/v1/workspaces/ws1/documents/doc1/docx",
      "/api/v1/workspaces/ws1/documents/doc1/pptx",
      "/api/v1/share/tok1/pdf",
      "/api/v1/share/tok1/pdf",
    ]);
    for (const r of s.requests.filter((r) => r.cookie !== null)) expect(r.cookie).toBe(s.session);
  });

  test.each([
    ["import-pending", "import job status pending"],
    ["body-missing-table", 'imported body lacks "table"'],
    ["edit-lost", "the Markdown edit is not in the body"],
    ["md-cache-public", "md: Cache-Control public"],
    ["pdf-truncated", "pdf export is not a complete PDF"],
    ["docx-without-edit", "docx export lacks the edit"],
    ["pptx-malformed", "unclosed tag: a:p"],
    ["export-without-auth", "HTTP 200, expected 401"],
    ["share-not-revoked", "/api/v1/share/tok1/pdf: HTTP 200, expected 404"],
    ["redirect-body", "/api/v1/share/tok1/pdf: HTTP 302, expected 200"],
  ] as const)("create refuses %s", async (fault, needle) => {
    const s = setup([fault]);
    const result = await s.run("create");
    expect(result.code).toBe(1);
    expect(result.stderr).toContain(needle);
    expect(s.requests.some((r) => r.path === "/elsewhere")).toBe(false);
  });

  test("restart refuses a changed body", async () => {
    const s = setup();
    expect((await s.run("create")).code).toBe(0);
    // Re-point the stand-in: the body it serves now differs from the saved one.
    await s.server.stop(true);
    const changed = startFakeServer(new Set<Fault>(["restart-body-changed"]));
    cleanups.push(() => changed.server.stop(true));
    // Async spawn: the stand-in answers on this process's event loop.
    const proc = Bun.spawn(
      [process.execPath, SCRIPT, changed.base, "ws1", s.jar, s.state, "restart"],
      { stdout: "ignore", stderr: "pipe" },
    );
    const [stderr, code] = await Promise.all([new Response(proc.stderr).text(), proc.exited]);
    expect(code).toBe(1);
    expect(stderr).toContain("the document body changed across the restart");
  });

  test("refuses an unknown phase and a jar without the session", async () => {
    const s = setup();
    expect((await s.run("other")).stderr).toContain("phase must be create or restart");
    writeFileSync(
      s.jar,
      "# Netscape HTTP Cookie File\n.other.example\tTRUE\t/\tFALSE\t0\tfvoci_session\tx\n",
    );
    const result = await s.run("create");
    expect(result.code).toBe(1);
    expect(result.stderr).toContain("no fvoci_session cookie for localhost");
    expect(s.requests).toHaveLength(0);
  });
});

describe("cookie jar", () => {
  test("reads curl's HttpOnly lines and keeps host-only cookies of the base host", () => {
    const jar = parseCookieJar(
      cookieJarText() + "localhost\tFALSE\t/\tFALSE\t0\ttheme\tdark\n",
      "jar",
    );
    expect(cookieHeader(jar, "http://localhost:8080", "jar")).toBe(
      "fvoci_session=s3cr3t; theme=dark",
    );
    expect(() => cookieHeader(jar, "http://127.0.0.1:8080", "jar")).toThrow("no fvoci_session");
  });

  test("refuses a file without the Netscape header or with a bad line", () => {
    expect(() => parseCookieJar("localhost\tFALSE\t/\tFALSE\t0\ta\tb\n", "jar")).toThrow(
      "Netscape",
    );
    expect(() =>
      parseCookieJar("# Netscape HTTP Cookie File\nlocalhost\tTRUE\t/\n", "jar"),
    ).toThrow("invalid");
    expect(() =>
      parseCookieJar("# Netscape HTTP Cookie File\nlocalhost\tTRUE\t/\tFALSE\t0\ta\tb\n", "jar"),
    ).toThrow("invalid");
  });
});

const utf8 = (text: string) => new TextEncoder().encode(text);
const docxWith = (xml: string) => writeZip([{ name: "word/document.xml", data: utf8(xml) }]);

// A DOCX holding the edit in an archive the original client refused (expat
// or zipfile); the smoke must fail on each, not find the text and pass.
function renamedLocalHeader(): Uint8Array {
  const zip = Buffer.from(docxWith("<a>후속 편집 저장</a>"));
  zip[30] = "X".charCodeAt(0); // first byte of the local file name; CRC and data unchanged
  return zip;
}
const refusedExports: Array<[string, Uint8Array, string]> = [
  ["attribute without value", docxWith("<a bad>후속 편집 저장</a>"), "attribute without value"],
  [
    "undefined entity in an attribute",
    docxWith('<a x="&unknown;">후속 편집 저장</a>'),
    "undefined entity",
  ],
  ["duplicate attribute", docxWith('<a x="1" x="2">후속 편집 저장</a>'), "duplicate attribute"],
  ["character reference outside Char", docxWith("<a>&#0;후속 편집 저장</a>"), "character entity"],
  ["local header names another file", renamedLocalHeader(), "local header names a different file"],
  [
    "the same part twice",
    writeZip([
      { name: "word/document.xml", data: utf8("<a>후속 편집 저장</a>") },
      { name: "word/document.xml", data: utf8("<a>wrong text</a>") },
    ]),
    "word/document.xml appears twice",
  ],
  // OPC forbids DTDs, and saxes does not check DTD syntax: every DOCTYPE,
  // well-formed or not, is refused.
  ...[
    "<!DOCTYPE>",
    "<!DOCTYPE a [garbage]>",
    "<!DOCTYPE a [<!ELEMENT a bogus>]>",
    "<!DOCTYPE a SYSTEM>",
  ].map((doctype): [string, Uint8Array, string] => [
    `a DOCTYPE ${doctype}`,
    docxWith(`${doctype}<a>후속 편집 저장</a>`),
    "DOCTYPE is not allowed",
  ]),
  // ZIP features this reader does not implement, or a broken extra field.
  ...(
    [
      ["compressed patched data (flag bit 5)", { flags: 0x20 }, "flags 0x0020 are not supported"],
      ["strong encryption (flag bit 6)", { flags: 0x40 }, "flags 0x0040 are not supported"],
      ["version needed 9.9", { version: 99 }, "needs ZIP version 9.9"],
      ["a truncated extra field", { extra: Buffer.from([0x99, 0x99, 0x01, 0x00]) }, "extra field"],
    ] as const
  ).map(([label, entry, needle]): [string, Uint8Array, string] => [
    label,
    rawZip([{ name: "word/document.xml", data: "<a>후속 편집 저장</a>", ...entry }]),
    needle,
  ]),
  [
    "a non-canonical part name",
    writeZip([{ name: "word/../document.xml", data: utf8("<a>후속 편집 저장</a>") }]),
    "non-canonical part name",
  ],
];

describe("smoke-documents refuses malformed Office exports", () => {
  test.each(refusedExports)("create fails on %s", async (_name, docx, needle) => {
    const s = setup([], docx);
    const result = await s.run("create");
    expect(result.code).toBe(1);
    expect(result.stderr).toContain(needle);
    expect(result.stdout).toBe("");
  });

  test("the same archive layout with a well-formed part passes", async () => {
    const result = await setup([], docxWith("<a><b>후속 편집 저장</b></a>")).run("create");
    expect(result.code, result.stderr).toBe(0);
  });
});

describe("export checks", () => {
  test("PDF needs the header and an EOF marker in the last KiB", () => {
    expect(isPdf(utf8("%PDF-1.7\n%%EOF\n"))).toBe(true);
    expect(isPdf(utf8("%PDF-1.7\n%%EOF" + " ".repeat(1100)))).toBe(false);
    expect(isPdf(utf8("PDF-1.7 %%EOF"))).toBe(false);
  });

  test("office text joins runs and every relevant part must parse", () => {
    const parts = (...xml: string[]) =>
      writeZip(xml.map((x, i) => ({ name: `word/part${String(i)}.xml`, data: utf8(x) })));
    expect(
      officeHasText(
        parts('<w:d xmlns:w="urn:w"><w:t>후속 </w:t><w:t>편집 저장</w:t></w:d>'),
        "docx",
        "후속 편집 저장",
      ),
    ).toBe(true);
    expect(officeHasText(parts("<d><t>a &amp; b</t></d>"), "docx", "a & b")).toBe(true);
    expect(officeHasText(parts("<d>text</d>"), "pptx", "text")).toBe(false);
    expect(() =>
      officeHasText(parts("<d>후속 편집 저장</d>", "<d><t>x</d>"), "docx", "후속 편집 저장"),
    ).toThrow("unexpected close tag");
  });

  test("xml text: comments and PIs dropped, CDATA kept, namespaces and entities enforced", () => {
    expect(xmlText('<?xml version="1.0"?><a><!-- no --><b>x</b><![CDATA[<y>]]>&#xAC00;</a>')).toBe(
      "x<y>가",
    );
    expect(xmlText("  <a>x</a>  ")).toBe("x");
    expect(xmlText(`<a k="x>y" j='/>'><b>t</b></a>`)).toBe("t");
    expect(xmlText('<w:a xmlns:w="u"><w:t xml:space="preserve">x</w:t></w:a>')).toBe("x");
    expect(() => xmlText("<a>&nbsp;</a>")).toThrow("undefined entity");
    expect(() => xmlText('<!DOCTYPE a [<!ENTITY e "boom">]><a>&e;</a>')).toThrow(
      "DOCTYPE is not allowed",
    );
    expect(() => xmlText("<a/><b/>")).toThrow("only one root");
    expect(() => xmlText("text<a/>")).toThrow("outside of root");
    expect(() => xmlText("<w:a><w:t>x</w:t></w:a>")).toThrow("unbound namespace prefix");
    expect(() => xmlText('<a xmlns:p="u" p:x="1" xmlns:q="u" q:x="2"/>')).toThrow(
      "duplicate attribute",
    );
  });
});

describe("zip structure", () => {
  const one = () => Buffer.from(docxWith("<a>x</a>"));
  const central = (zip: Buffer, from = 0) =>
    zip.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]), from);
  const flip = (zip: Buffer, at: number) => {
    zip[at] = (zip[at] ?? 0) ^ 0xff;
  };

  test("round-trips a UTF-8 name", () => {
    const [entry] = readZip(writeZip([{ name: "설치 검증.md", data: utf8("# 제목\n") }]));
    expect(entry?.name).toBe("설치 검증.md");
    expect(new TextDecoder().decode(entry?.data)).toBe("# 제목\n");
  });

  test("refuses headers that disagree, bad CRCs and broken bounds", () => {
    const crcBoth = one();
    flip(crcBoth, 14);
    flip(crcBoth, central(crcBoth) + 16);
    expect(() => openZip(crcBoth).read("word/document.xml")).toThrow("bad CRC");
    const crcCentral = one();
    flip(crcCentral, central(crcCentral) + 16);
    expect(() => openZip(crcCentral)).toThrow("local header disagrees");
    expect(() => openZip(Buffer.concat([one(), Buffer.from("trailing")]))).toThrow(
      "no end of central directory",
    );
    expect(() => openZip(one().subarray(1))).toThrow("zip:");
    const shortDir = one();
    shortDir.writeUInt32LE(shortDir.readUInt32LE(shortDir.length - 10) - 1, shortDir.length - 10);
    expect(() => openZip(shortDir)).toThrow("central directory");
  });

  test("refuses overlapping entries and archives over the total budget", () => {
    const zip = writeZip([
      { name: "a.xml", data: utf8("<a>1</a>") },
      { name: "b.xml", data: utf8("<a>1</a>") },
    ]);
    const overlapped = Buffer.from(zip);
    const second = central(overlapped, central(overlapped) + 4);
    overlapped.writeUInt32LE(0, second + 42); // b.xml's local offset now points at a.xml
    expect(() => openZip(overlapped)).toThrow("zip:");
    expect(() => openZip(zip, 15)).toThrow("in total");
    expect(openZip(zip, 16).names).toEqual(["a.xml", "b.xml"]);
  });

  test("an entry inflating past its declared size is refused", () => {
    const zip = Buffer.from(docxWith("<a>" + "x".repeat(4096) + "</a>"));
    zip.writeUInt32LE(16, central(zip) + 24);
    zip.writeUInt32LE(16, 22);
    expect(() => openZip(zip).read("word/document.xml")).toThrow();
  });

  // Archives written by zip 8.6.0 (the crate src/documents/{docx,pptx}.rs
  // use: ZipWriter over a Cursor, Deflated; and Stored with a directory entry
  // and a UTF-8 name) must pass unchanged.
  test.each(["zip-8.6.0-deflated.docx", "zip-8.6.0-stored-dirs.docx"])(
    "accepts genuine %s",
    (file) => {
      const bytes = readFileSync(join(import.meta.dir, "testdata", file));
      const archive = openZip(bytes);
      for (const name of archive.names) archive.read(name);
      expect(officeHasText(bytes, "docx", "후속 편집 저장")).toBe(true);
    },
  );
});

type RawEntry = {
  name: string;
  data?: string;
  nameBytes?: Uint8Array;
  flags?: number;
  packed?: Uint8Array;
  descriptor?: boolean;
  descriptorCrc?: number;
  version?: number; // version needed to extract, local and central
  extra?: Uint8Array; // extra field, local and central
  localFlags?: number; // local header flags when they differ from the central ones
};

// A hand-assembled archive for structural cases writeZip cannot express.
function rawZip(entries: RawEntry[], options: { gap?: number; secondEnd?: boolean } = {}) {
  const parts: Buffer[] = [Buffer.alloc(options.gap ?? 0)];
  const centrals: Buffer[] = [];
  let offset = options.gap ?? 0;
  for (const entry of entries) {
    const data = Buffer.from(entry.data ?? "");
    const name = Buffer.from(entry.nameBytes ?? Buffer.from(entry.name));
    const packed = Buffer.from(entry.packed ?? deflateRawSync(data));
    const crc = crc32(data);
    const flags = (entry.flags ?? 0) | (entry.descriptor ? 8 : 0);
    const version = entry.version ?? 20;
    const extra = Buffer.from(entry.extra ?? []);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(version, 4);
    local.writeUInt16LE(entry.localFlags ?? flags, 6);
    local.writeUInt16LE(8, 8);
    if (!entry.descriptor) {
      local.writeUInt32LE(crc, 14);
      local.writeUInt32LE(packed.length, 18);
      local.writeUInt32LE(data.length, 22);
    }
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(extra.length, 28);
    const descriptor = Buffer.alloc(entry.descriptor ? 16 : 0);
    if (entry.descriptor) {
      descriptor.writeUInt32LE(0x08074b50, 0);
      descriptor.writeUInt32LE(entry.descriptorCrc ?? crc, 4);
      descriptor.writeUInt32LE(packed.length, 8);
      descriptor.writeUInt32LE(data.length, 12);
    }
    parts.push(local, name, extra, packed, descriptor);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(version, 6);
    central.writeUInt16LE(flags, 8);
    central.writeUInt16LE(8, 10);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(packed.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt16LE(extra.length, 30);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, name, extra);
    offset += 30 + name.length + extra.length + packed.length + descriptor.length;
  }
  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  if (!options.secondEnd) return Buffer.concat([...parts, directory, end]);
  // The real end record's comment holds a second record that also reaches EOF.
  end.writeUInt16LE(22, 20);
  const inner = Buffer.from(end);
  inner.writeUInt16LE(0, 20);
  return Buffer.concat([...parts, directory, end, inner]);
}

describe("zip structure (assembled archives)", () => {
  const doc = { name: "word/document.xml", data: "<a>후속 편집 저장</a>" };

  test("a well-formed assembled archive, with and without a data descriptor, passes", () => {
    for (const archive of [
      rawZip([doc]),
      rawZip([{ ...doc, descriptor: true }]),
      // Deflate option bits 1-2 (Word writes 0x0006), version 1.0, a timestamp extra field.
      rawZip([{ ...doc, flags: 0x06, version: 10, extra: Buffer.from("555405000100000000", "hex") }]),
    ]) {
      expect(officeHasText(archive, "docx", "후속 편집 저장")).toBe(true);
    }
  });

  test.each([
    [
      "bytes after the deflate stream",
      rawZip([
        {
          ...doc,
          packed: Buffer.concat([deflateRawSync(Buffer.from(doc.data)), Buffer.from("X")]),
        },
      ]),
      "does not end with the deflate stream",
    ],
    [
      "a data descriptor that disagrees",
      rawZip([{ ...doc, descriptor: true, descriptorCrc: 1 }]),
      "disagrees",
    ],
    ["unlisted bytes before the first entry", rawZip([doc], { gap: 64 }), "unlisted bytes"],
    [
      "an invalid UTF-8 name",
      rawZip([{ ...doc, nameBytes: Buffer.from([0x77, 0xff, 0x2e, 0x78]), flags: 0x800 }]),
      "not valid UTF-8",
    ],
    [
      "a non-ASCII name without the UTF-8 flag",
      rawZip([{ ...doc, nameBytes: Buffer.from("word/설치.xml") }]),
      "without the UTF-8 flag",
    ],
    [
      "the same part in another case",
      rawZip([doc, { name: "Word/Document.xml", data: "<a>wrong</a>" }]),
      "appears twice",
    ],
    ["two end records", rawZip([doc], { secondEnd: true }), "more than one end"],
    [
      "an unsupported flag in the local header only",
      rawZip([{ ...doc, localFlags: 0x20 }]),
      "flags 0x0020 are not supported",
    ],
    [
      "deflate option bits that differ between the headers",
      rawZip([{ ...doc, flags: 0x06, localFlags: 0x02 }]),
      "disagrees",
    ],
    [
      "extra-field bytes after the last record",
      rawZip([{ ...doc, extra: Buffer.from("55540100010000", "hex") }]),
      "extra field",
    ],
  ] as const)("refuses %s", (_name, archive, needle) => {
    expect(() => officeHasText(archive, "docx", "후속 편집 저장")).toThrow(needle);
  });

  test("an XML 1.1 part is refused", () => {
    const archive = rawZip([
      { name: "word/document.xml", data: '<?xml version="1.1"?><a>&#1;후속 편집 저장</a>' },
    ]);
    expect(() => officeHasText(archive, "docx", "후속 편집 저장")).toThrow("XML version 1.1");
  });
});
