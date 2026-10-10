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

  test("HWP FileHeader resolves through the CFB mini stream (MS-CFB 2.6.3)", () => {
    const bytes = buildHwp();
    const v = new DataView(bytes.buffer, bytes.byteOffset);
    const u32 = (at: number) => v.getUint32(at, true);
    const ENDOFCHAIN = 0xfffffffe;
    expect([...bytes.subarray(0, 8)]).toEqual([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    const sectorAt = (n: number) => 512 * (n + 1);
    const fatNext = (n: number) => u32(sectorAt(u32(76)) + n * 4); // single FAT sector (DIFAT[0])
    const dirEntry = (i: number) => sectorAt(u32(48)) + i * 128;
    const root = dirEntry(0);
    const file = dirEntry(1);
    expect(String.fromCharCode(...new Uint16Array(bytes.slice(file, file + 20).buffer))).toBe(
      "FileHeader",
    );
    const size = Number(v.getBigUint64(file + 120, true));
    expect(size).toBe(256);
    expect(size).toBeLessThan(u32(56)); // below the cutoff, so it must be a mini stream
    // Mini stream container: the Root Entry's regular-sector chain.
    const container: number[] = [];
    for (let s = u32(root + 116); s !== ENDOFCHAIN; s = fatNext(s)) container.push(s);
    expect(container.length * 512).toBeGreaterThanOrEqual(Number(v.getBigUint64(root + 120, true)));
    // FileHeader: the mini FAT chain from its start, read out of the container.
    const miniFat = sectorAt(u32(60));
    expect(u32(64)).toBe(1);
    const out: number[] = [];
    for (let m = u32(file + 116); m !== ENDOFCHAIN; m = u32(miniFat + m * 4)) {
      const at = sectorAt(container[Math.floor((m * 64) / 512)]!) + ((m * 64) % 512);
      out.push(...bytes.subarray(at, at + 64));
    }
    expect(out.length).toBe(size);
    expect(strFromU8(Uint8Array.from(out.slice(0, 17)))).toBe("HWP Document File");
  });

  test("HWP unused directory entries are zero apart from NOSTREAM links (MS-CFB 2.6.3)", () => {
    const bytes = buildHwp();
    const dir = 512 * (new DataView(bytes.buffer, bytes.byteOffset).getUint32(48, true) + 1);
    let unused = 0;
    for (let at = dir; at < dir + 512; at += 128) {
      if (bytes[at + 66] !== 0) continue;
      unused++;
      const entry = [...bytes.subarray(at, at + 128)];
      expect(entry.slice(68, 80)).toEqual(Array<number>(12).fill(0xff));
      expect([...entry.slice(0, 68), ...entry.slice(80)]).toEqual(Array<number>(116).fill(0));
    }
    expect(unused).toBe(2);
  });

  test("writes the four specimens into an empty directory", () => {
    const root = mkdtempSync(join(tmpdir(), "compat-gen-"));
    try {
      const out = join(root, "new");
      const r = run(["--output-dir", out]);
      expect(r.code).toBe(0);
      expect(r.stdout).toBe(
        "sample.pdf 593 bytes\nsample.docx 949 bytes\nsample.hwpx 1717 bytes\nsample.hwp 2560 bytes\n",
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
        ["--output", join(root, "abbreviated")],
        ["--output-dir", full],
        ["--output-dir", file],
        ["--output-dir", here],
      ]) {
        expect(run(args).code).toBe(2);
      }
      expect(readdirSync(full)).toEqual(["keep"]);
      expect(readdirSync(root).sort()).toEqual(["file", "full"]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
