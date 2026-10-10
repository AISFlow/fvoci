import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type Fault, cookieJarText, startFakeServer } from "./smoke-documents.fixture.ts";
import { cookieHeader, isPdf, officeHasText, parseCookieJar } from "./smoke-documents.ts";
import { xmlText } from "./xml-text.ts";
import { readZip, writeZip } from "./zip.ts";

const SCRIPT = join(import.meta.dir, "smoke-documents.ts");
const cleanups: Array<() => Promise<void>> = [];
afterEach(async () => {
  for (const cleanup of cleanups.splice(0)) await cleanup();
});

function setup(faults: Fault[] = []) {
  const fake = startFakeServer(new Set(faults));
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
    ["pptx-malformed", "unclosed XML element"],
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

describe("export checks", () => {
  test("PDF needs the header and an EOF marker in the last KiB", () => {
    expect(isPdf(new TextEncoder().encode("%PDF-1.7\n%%EOF\n"))).toBe(true);
    expect(isPdf(new TextEncoder().encode("%PDF-1.7\n%%EOF" + " ".repeat(1100)))).toBe(false);
    expect(isPdf(new TextEncoder().encode("PDF-1.7 %%EOF"))).toBe(false);
  });

  test("office text joins runs and every part must parse", () => {
    const part = (xml: string) =>
      writeZip([{ name: "word/document.xml", data: new TextEncoder().encode(xml) }]);
    expect(
      officeHasText(
        part('<w:d xmlns:w="urn:w"><w:t>후속 </w:t><w:t>편집 저장</w:t></w:d>'),
        "docx",
        "후속 편집 저장",
      ),
    ).toBe(true);
    expect(
      officeHasText(part('<w:d xmlns:w="urn:w"><w:t>a &amp; b</w:t></w:d>'), "docx", "a & b"),
    ).toBe(true);
    expect(officeHasText(part('<w:d xmlns:w="urn:w">text</w:d>'), "pptx", "text")).toBe(false);
    expect(() => officeHasText(part('<w:d xmlns:w="urn:w"><w:t>x</w:d>'), "docx", "x")).toThrow(
      "mismatched",
    );
  });

  test("xml text skips comments and processing instructions and keeps CDATA", () => {
    expect(xmlText('<?xml version="1.0"?><a><!-- no --><b>x</b><![CDATA[<y>]]>&#xAC00;</a>')).toBe(
      "x<y>가",
    );
    expect(() => xmlText("<a>&nbsp;</a>")).toThrow("undefined XML entity");
    expect(() => xmlText("<a/><b/>")).toThrow("more than one root");
    expect(() => xmlText("<w:a><w:t>x</w:t></w:a>")).toThrow("unbound prefix");
    expect(xmlText(`<a k="x>y" j='/>'><b>t</b></a>`)).toBe("t");
    expect(() => xmlText('<a xmlns:w="u"><b w:x="1" v:y="2"/></a>')).toThrow("unbound prefix");
    expect(xmlText('<w:a xmlns:w="u"><w:t xml:space="preserve">x</w:t></w:a>')).toBe("x");
  });

  test("zip round-trips a UTF-8 name and refuses a corrupted entry", () => {
    const zip = writeZip([{ name: "설치 검증.md", data: new TextEncoder().encode("# 제목\n") }]);
    const [entry] = readZip(zip);
    expect(entry?.name).toBe("설치 검증.md");
    expect(new TextDecoder().decode(entry?.data)).toBe("# 제목\n");
    const broken = Buffer.from(zip);
    const centralCrc = broken.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02])) + 16;
    broken[centralCrc] = (broken[centralCrc] ?? 0) ^ 0xff;
    expect(() => readZip(broken)).toThrow("bad CRC");
  });
});
