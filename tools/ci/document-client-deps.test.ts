import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { leakedPackages, main } from "./document-client-deps.ts";

const SCRIPT = join(import.meta.dir, "document-client-deps.ts");
const encoder = new TextEncoder();

function metadata(...names: string[]) {
  return {
    packages: ["document-extract-client", "serde", ...names].map((name) => ({
      name,
      version: "1.0.0",
    })),
    resolve: null,
  };
}

function run(argv: string[], files: Record<string, string | Uint8Array> = {}): number {
  return main(argv, {
    read: (path) => {
      const file = files[path];
      if (file === undefined) throw new Error(`ENOENT: ${path}`);
      return typeof file === "string" ? encoder.encode(file) : file;
    },
    err: () => {},
  });
}

describe("leakedPackages", () => {
  test("the thin client's packages pass", () => {
    expect(leakedPackages(metadata())).toEqual([]);
  });

  // The native parser stack, spelled out here so the list cannot shrink unnoticed.
  for (const name of ["rhwp", "cfb", "zip", "flate2", "skia-safe", "skia-bindings"]) {
    test(`${name} is refused`, () => {
      expect(leakedPackages(metadata(name))).toEqual([name]);
    });
  }

  test("every leaked package is named once, sorted", () => {
    expect(leakedPackages(metadata("zip", "cfb", "zip", "rhwp"))).toEqual(["cfb", "rhwp", "zip"]);
  });

  test("names match exactly, as cargo spells them", () => {
    expect(leakedPackages(metadata("skia_safe", "zip-extra", "Zip"))).toEqual([]);
  });

  test("a package only in resolve is not in the package list", () => {
    const value = { ...metadata(), resolve: { nodes: [{ id: "zip 2.0.0" }] } };
    expect(leakedPackages(value)).toEqual([]);
  });

  for (const [label, value] of [
    ["a list", []],
    ["null", null],
    ["no packages", { resolve: null }],
    ["packages not a list", { packages: { zip: {} } }],
    ["empty packages", { packages: [] }],
    ["a package without a name", { packages: [{ version: "1" }] }],
    ["a non-string name", { packages: [{ name: 7 }] }],
    ["a non-object package", { packages: ["zip"] }],
  ] as const) {
    test(`${label} is not cargo metadata`, () => {
      expect(() => leakedPackages(value)).toThrow();
    });
  }
});

describe("main", () => {
  test("exit 0 for clean metadata", () => {
    expect(run(["m.json"], { "m.json": JSON.stringify(metadata()) })).toBe(0);
  });

  test("exit 1 for a leak, a missing file, bad JSON, a BOM or bad UTF-8", () => {
    expect(run(["m.json"], { "m.json": JSON.stringify(metadata("flate2")) })).toBe(1);
    expect(run(["missing.json"])).toBe(1);
    expect(run(["m.json"], { "m.json": '{"packages": [' })).toBe(1);
    expect(run(["m.json"], { "m.json": "﻿" + JSON.stringify(metadata()) })).toBe(1);
    expect(run(["m.json"], { "m.json": new Uint8Array([0x7b, 0xff, 0x7d]) })).toBe(1);
    expect(run(["m.json"], { "m.json": '{"packages": [{"name": NaN}]}' })).toBe(1);
  });

  test("exit 2 without exactly one path", () => {
    expect(run([])).toBe(2);
    expect(run(["a.json", "b.json"])).toBe(2);
    expect(run(["--help"])).toBe(2);
  });

  test("the CLI reports the leak on stderr", () => {
    const dir = mkdtempSync(join(tmpdir(), "document-client-deps-"));
    try {
      const path = join(dir, "metadata.json");
      writeFileSync(path, JSON.stringify(metadata("skia-bindings", "rhwp")));
      const proc = Bun.spawnSync([process.execPath, SCRIPT, path]);
      expect(proc.exitCode).toBe(1);
      expect(proc.stdout.toString()).toBe("");
      expect(proc.stderr.toString()).toBe(
        "document-client-deps: native dependencies leaked into process client: rhwp, skia-bindings\n",
      );
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
