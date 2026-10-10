import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { strFromU8, unzipSync } from "fflate";
import { buildDocx, buildHwp, buildHwpx, buildPdf } from "./gen.ts";

const here = import.meta.dirname;
const gen = join(here, "gen.ts");

function run(args: string[]) {
  const p = Bun.spawnSync([process.execPath, gen, ...args], { stdout: "pipe", stderr: "pipe" });
  return { code: p.exitCode, stdout: p.stdout.toString(), stderr: p.stderr.toString() };
}

describe("compat fixture generator", () => {
  test("reproduces the checked-in generated fixtures byte for byte", () => {
    expect(Buffer.compare(buildPdf(), readFileSync(join(here, "sample.pdf")))).toBe(0);
    expect(Buffer.compare(buildDocx(), readFileSync(join(here, "sample.docx")))).toBe(0);
  });

  test("HWPX is an OCF package with a leading stored mimetype", () => {
    const bytes = buildHwpx();
    // Local header 1: method 0 (stored), name "mimetype", body right after it.
    const v = new DataView(bytes.buffer, bytes.byteOffset);
    expect(v.getUint16(8, true)).toBe(0);
    expect(strFromU8(bytes.subarray(30, 38))).toBe("mimetype");
    expect(strFromU8(bytes.subarray(38, 57))).toBe("application/hwp+zip");
    const files = unzipSync(bytes);
    expect(Object.keys(files)).toEqual([
      "mimetype",
      "version.xml",
      "META-INF/container.xml",
      "META-INF/manifest.xml",
      "Contents/content.hpf",
      "Contents/header.xml",
      "Contents/section0.xml",
    ]);
    expect(strFromU8(files["Contents/section0.xml"]!)).toContain(
      "<hp:t>한글본문색인토큰 HWPX</hp:t>",
    );
  });

  test("HWP is a CFB v3 file whose FileHeader stream carries the HWP signature", () => {
    const bytes = buildHwp();
    expect(bytes.length).toBe(2048);
    expect([...bytes.subarray(0, 8)]).toEqual([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    const v = new DataView(bytes.buffer, bytes.byteOffset);
    expect(v.getUint32(512 + 8, true)).toBe(0xfffffffe); // FAT: stream sector 2 ends its chain
    const entry = 1024 + 128; // directory sector 1, entry 1
    expect(String.fromCharCode(...new Uint16Array(bytes.slice(entry, entry + 20).buffer))).toBe(
      "FileHeader",
    );
    expect(v.getUint32(entry + 116, true)).toBe(2);
    expect(strFromU8(bytes.subarray(1536, 1536 + 17))).toBe("HWP Document File");
  });

  test("writes the four specimens into an empty directory", () => {
    const root = mkdtempSync(join(tmpdir(), "compat-gen-"));
    try {
      const out = join(root, "new");
      const r = run(["--output-dir", out]);
      expect(r.code).toBe(0);
      expect(r.stdout).toBe(
        "sample.pdf 593 bytes\nsample.docx 949 bytes\nsample.hwpx 1717 bytes\nsample.hwp 2048 bytes\n",
      );
      expect(readdirSync(out).sort()).toEqual([
        "sample.docx",
        "sample.hwp",
        "sample.hwpx",
        "sample.pdf",
      ]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("refuses missing, nonempty, non-directory and checked-in output paths", () => {
    const root = mkdtempSync(join(tmpdir(), "compat-gen-"));
    try {
      const full = join(root, "full");
      mkdirSync(full);
      writeFileSync(join(full, "keep"), "x");
      const file = join(root, "file");
      writeFileSync(file, "x");
      for (const args of [
        [],
        ["--output-dir", ""],
        ["--bogus"],
        ["--output-dir", full],
        ["--output-dir", file],
        ["--output-dir", here],
      ]) {
        expect(run(args).code).toBe(2);
      }
      expect(readdirSync(full)).toEqual(["keep"]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
