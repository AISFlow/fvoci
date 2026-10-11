import { describe, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { argumentErrors, checkComposeConfig, declaresBuildSha, render } from "./dist-check.ts";
import { dumpJson, jsonEqual, repr, sorted, splitLines } from "./py.ts";
import { fillEnv, indexPlatforms, jsonGet, searchHas, setEnv, urlQuote } from "./smoke-checks.ts";
import { readSources, versionErrors } from "./version.ts";

const ROOT = resolve(import.meta.dir, "../..");

describe("message and record formatting", () => {
  test("names are quoted as the release messages always quoted them", () => {
    expect(repr(["ROOMS", "A'B", 'C"D'])).toBe(`['ROOMS', "A'B", 'C"D']`);
    expect(repr([])).toBe("[]");
    expect(repr("tab\there\u0001")).toBe("'tab\\there\\x01'");
    expect(repr([null, true, 3])).toBe("[None, True, 3]");
  });

  test("records keep the published JSON layout", () => {
    expect(dumpJson({ a: [], b: {}, c: ["x", 1], d: "é🎉" }, { indent: 2 })).toBe(
      '{\n  "a": [],\n  "b": {},\n  "c": [\n    "x",\n    1\n  ],\n  "d": "\\u00e9\\ud83c\\udf89"\n}',
    );
    expect(dumpJson({ a: [1, "é"], b: null }, { ascii: false })).toBe('{"a": [1, "é"], "b": null}');
  });

  // Stricter than the Python oracle: only LF ends a line, and true is not 1.
  test("lines and equality", () => {
    expect(splitLines("a\nb\fc\n\nd\n")).toEqual(["a", "b\fc", "", "d"]);
    expect(splitLines("")).toEqual([]);
    expect(jsonEqual({ a: [1, { b: true }] }, { a: [1, { b: 1 }] })).toBe(false);
    expect(jsonEqual({ a: 1, b: { c: [true] } }, { b: { c: [true] }, a: 1 })).toBe(true);
    expect(jsonEqual({ a: [1, 2] }, { a: [2, 1] })).toBe(false);
  });

  test("names sort by code point", () => {
    expect(sorted(["\u{1F600}", "Ａ", "B", "A_X", "A"])).toEqual([
      "A",
      "A_X",
      "B",
      "Ａ",
      "\u{1F600}",
    ]);
  });
});

describe("render contract", () => {
  const args = {
    version: "0.2.3",
    sha: "a".repeat(40),
    repository: "o/n",
    image: "ghcr.io/aisflow/fvoci",
    indexDigest: "sha256:" + "1".repeat(64),
    amd64Digest: "sha256:" + "2".repeat(64),
    arm64Digest: "sha256:" + "3".repeat(64),
    runUrl: "https://x/",
  };
  const sources = {
    composeSource: "c.yml",
    envSource: "c.env.example",
    guideSource: "c.INSTALL.md",
    compose:
      "x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:0.1.0}\nservices:\n  app:\n    image: *fvoci-image\n    environment:\n      K: ${K:?set K}\n",
    env: "K=\n",
    guide: "guide\n",
    notesTemplate: "@VERSION@ @VERSION@ @IMAGE_REF@\n",
  };

  test("every argument error is reported together", () => {
    expect(
      argumentErrors({
        ...args,
        version: "1.0.0",
        sha: "x",
        indexDigest: "sha256:1",
        runUrl: "http://x",
      }),
    ).toEqual([
      "version '1.0.0' is not 0.y.z",
      "--sha must be a full commit SHA",
      "index_digest must be sha256:<64 hex>",
      "--run-url must be an https URL",
    ]);
  });

  test("replaces every placeholder and stops after release.json when one is left", () => {
    const ok = render(args, sources);
    expect(ok.error).toBeUndefined();
    const notes = ok.files.find(([name]) => name === "RELEASE-NOTES.md")?.[1];
    expect(notes).toBe(`0.2.3 0.2.3 ghcr.io/aisflow/fvoci:0.2.3@sha256:${"1".repeat(64)}\n`);
    const left = render(args, { ...sources, notesTemplate: "@NEW@ @OTHER@ @NEW@" });
    expect(left.error).toBe("release notes placeholders left: ['@NEW@', '@OTHER@']");
    expect(left.files.map(([name]) => name)).toEqual([
      "compose.yml",
      "env.example",
      "INSTALL.md",
      "release.json",
    ]);
  });

  test("refuses a variable assigned twice and env_file", () => {
    expect(render(args, { ...sources, env: "K=\nK=\n" }).error).toBe(
      "c.env.example: a variable is assigned twice",
    );
    expect(render(args, { ...sources, compose: sources.compose + "    env_file: x\n" }).error).toBe(
      "c.yml: the release compose must not use env_file",
    );
  });

  test("the Dockerfile ARG counts only inside rust-build", () => {
    expect(declaresBuildSha("from a as Rust-Build\n  ARG FVOCI_BUILD_SHA\n")).toBe(true);
    expect(declaresBuildSha("FROM a AS rust-build\nARG FVOCI_BUILD_SHA_X\n")).toBe(false);
    expect(declaresBuildSha("FROM a AS other\nARG FVOCI_BUILD_SHA=\n")).toBe(false);
  });

  test("compose config: product app, postgres, no env_file", () => {
    const image = "img@sha256:0";
    const report = checkComposeConfig(
      {
        services: {
          app: { image, ports: [{ target: 8080 }], environment: { A: "preflight-K" } },
          worker: { image: "other", env_file: [{ path: ".env" }] },
        },
      },
      image,
    );
    expect(report.summary).toBe("product image services: ['app']; app: ['app']");
    expect(report.problems).toEqual(["no postgres service", "worker needs an env_file"]);
  });
});

describe("release-smoke helpers", () => {
  test("json paths index lists by digit keys only", () => {
    const value = { items: [{ id: "a" }], n: 3 };
    expect(jsonGet(value, ["items", "0", "id"])).toBe("a");
    expect(() => jsonGet(value, ["0"])).toThrow();
    expect(() => jsonGet(value, ["items", "1"])).toThrow("out of range");
  });

  test("index platforms skip attestation manifests", () => {
    expect(
      indexPlatforms({
        manifests: [
          { platform: { os: "linux", architecture: "amd64" }, digest: "d1" },
          { platform: { os: "unknown", architecture: "unknown" }, digest: "att" },
        ],
      }),
    ).toEqual({ "linux/amd64": "d1" });
  });

  test("fill-env fills empty entries and keyrings under their active id", () => {
    const text = "# c\nA=\nX_ACTIVE_KEY_ID=install\nX_KEYS=\nB=kept\n";
    let n = 0;
    const out = fillEnv(text, () => `t${String(++n)}`);
    expect(out).toBe('# c\nA=t1\nX_ACTIVE_KEY_ID=install\nX_KEYS={"install":"t2"}\nB=kept\n');
    expect(() => fillEnv("Y_KEYS=\n", () => "t")).toThrow("no active key id for Y_KEYS");
  });

  test("set-env replaces exactly one line", () => {
    expect(setEnv("A=1\nB=2\n", "B", '{"k":"v"}')).toBe('A=1\nB={"k":"v"}\n');
    expect(() => setEnv("A=1\n", "B", "x")).toThrow();
    expect(() => setEnv("B=1\nB=2\n", "B", "x")).toThrow();
  });

  test("url-quote keeps unreserved bytes and slash", () => {
    expect(urlQuote("Release smoke a1/b~c")).toBe("Release%20smoke%20a1/b~c");
    expect(urlQuote("한!*'()")).toBe("%ED%95%9C%21%2A%27%28%29");
  });

  test("search matches documentId or id", () => {
    expect(searchHas({ items: [{ documentId: "d" }] }, "d")).toBe(true);
    expect(searchHas({ items: [{ id: "d" }] }, "d")).toBe(true);
    expect(searchHas({ items: [{ title: "d" }] }, "d")).toBe(false);
  });
});

describe("version policy", () => {
  function tree(edit: (root: string) => void): string {
    const root = mkdtempSync(join(tmpdir(), "fvoci-release-version-"));
    for (const rel of [
      "Cargo.toml",
      "Cargo.lock",
      "apps/web/openapi.json",
      "apps/web/package.json",
      "src/api/openapi.rs",
    ]) {
      mkdirSync(dirname(join(root, rel)), { recursive: true });
      cpSync(join(ROOT, rel), join(root, rel));
    }
    edit(root);
    return root;
  }
  const version = String(versionErrors(readSources(ROOT), "", "").version);

  test("the tree agrees with Cargo.toml", () => {
    const result = versionErrors(readSources(ROOT), `v${version}`, version);
    expect(result.errors).toEqual([]);
    expect(version).toMatch(/^0\.\d+\.\d+$/);
  });

  test("every mismatch is reported", () => {
    const root = tree((r) => {
      const replace = (rel: string, from: string, to: string) => {
        const text = readFileSync(join(r, rel), "utf8");
        expect(text).toContain(from);
        writeFileSync(join(r, rel), text.replace(from, to));
      };
      replace("apps/web/openapi.json", `"version": "${version}"`, '"version": "9.9.9"');
      replace(
        "apps/web/package.json",
        '"private": true,',
        '"private": true,\n  "version": "0.0.1",',
      );
    });
    try {
      expect(versionErrors(readSources(root), "v1.0.0", "0.0.0").errors).toEqual([
        `apps/web/openapi.json info.version '9.9.9' != ${version}`,
        `apps/web/package.json version '0.0.1' != ${version}`,
        `tag 'v1.0.0' != v${version}`,
        `image tag '0.0.0' != ${version}`,
      ]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
