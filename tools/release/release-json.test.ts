import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { RELEASE_ASSETS } from "./fixtures.ts";
import { run } from "./release-json.ts";

const SHA = "a".repeat(40);
const DIGEST = "sha256:" + "d".repeat(64);
const RECORD = {
  version: "0.1.0",
  sourceSha: SHA,
  indexDigest: DIGEST,
  image: `ghcr.io/aisflow/fvoci:0.1.0@${DIGEST}`,
};

function withFile(content: unknown, body: (path: string) => void) {
  const dir = mkdtempSync(join(tmpdir(), "release-json-"));
  try {
    const path = join(dir, "file.json");
    writeFileSync(
      path,
      typeof content === "string" || content instanceof Uint8Array
        ? content
        : JSON.stringify(content),
    );
    body(path);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const refusal = (argv: string[]) => {
  try {
    run(argv);
  } catch (error) {
    return error as Error & { code: number };
  }
  throw new Error(`accepted ${JSON.stringify(argv)}`);
};

describe("release.json", () => {
  test("version matches the tag", () => {
    withFile(RECORD, (path) => {
      expect(run(["version", path, "v0.1.0"])).toBe("0.1.0");
      expect(refusal(["version", path, "v0.1.1"]).code).toBe(1);
    });
  });

  test("string fields", () => {
    withFile(RECORD, (path) => {
      expect(run(["field", path, "indexDigest"])).toBe(DIGEST);
      expect(run(["field", path, "image"])).toBe(RECORD.image);
      expect(refusal(["field", path, "missing"]).code).toBe(1);
    });
    withFile("not json", (path) => {
      expect(refusal(["field", path, "image"]).message).toContain("is not JSON");
    });
    expect(refusal(["field", "/nonexistent/release.json", "image"]).message).toContain("ENOENT");
  });

  test("record-digest requires the same version and source SHA", () => {
    withFile(RECORD, (path) => {
      expect(run(["record-digest", path, "0.1.0", SHA])).toBe(DIGEST);
      expect(refusal(["record-digest", path, "0.1.0", "b".repeat(40)]).message).toBe(
        `existing release records 0.1.0 at ${SHA}, not 0.1.0 at ${"b".repeat(40)}`,
      );
      expect(refusal(["record-digest", path, "0.1.1", SHA]).code).toBe(1);
    });
    withFile({ version: "0.1.0" }, (path) => {
      expect(refusal(["record-digest", path, "0.1.0", SHA]).message).toBe(
        `existing release records 0.1.0 at None, not 0.1.0 at ${SHA}`,
      );
    });
  });
});

describe("release state", () => {
  const published = JSON.stringify({ state: "published", assets: RELEASE_ASSETS });

  test("state", () => {
    expect(run(["state", published])).toBe("published");
    expect(run(["state", '{"state": "none", "assets": []}'])).toBe("none");
    expect(refusal(["state", "[]"]).code).toBe(1);
  });

  test("assets-complete names the missing assets", () => {
    expect(run(["assets-complete", published, ...RELEASE_ASSETS])).toBeUndefined();
    const partial = JSON.stringify({ state: "published", assets: ["compose.yml"] });
    expect(
      refusal(["assets-complete", partial, "release.json", "compose.yml", "SHA256SUMS"]).message,
    ).toBe("missing ['SHA256SUMS', 'release.json']");
  });
});

describe("image-labels", () => {
  const labels = (version: string, revision: string) => ({
    config: {
      Labels: {
        "org.opencontainers.image.version": version,
        "org.opencontainers.image.revision": revision,
      },
    },
  });

  test("both platforms labelled with this version and SHA", () => {
    withFile(
      { "linux/amd64": labels("0.1.0", SHA), "linux/arm64": labels("0.1.0", SHA) },
      (path) => {
        expect(run(["image-labels", path, "0.1.0", SHA])).toBeUndefined();
      },
    );
  });

  test.each([
    [{ "linux/amd64": labels("0.1.0", SHA) }],
    [{ "linux/amd64": labels("0.1.0", SHA), "linux/arm64": labels("0.1.0", "b".repeat(40)) }],
    [{ "linux/amd64": labels("0.0.9", SHA), "linux/arm64": labels("0.1.0", SHA) }],
    [{ "linux/amd64": { config: {} }, "linux/arm64": labels("0.1.0", SHA) }],
    [
      {
        "linux/amd64": labels("0.1.0", SHA),
        "linux/arm64": labels("0.1.0", SHA),
        "linux/s390x": labels("0.1.0", SHA),
      },
    ],
  ])("refuses %j", (images) => {
    withFile(images, (path) => {
      expect(refusal(["image-labels", path, "0.1.0", SHA]).code).toBe(1);
    });
  });
});

test("usage errors", () => {
  expect(refusal(["nope"]).code).toBe(2);
  expect(refusal(["version", "only-one"]).code).toBe(2);
  expect(refusal(["assets-complete"]).code).toBe(2);
});

describe("review counterexamples", () => {
  const labels = {
    config: {
      Labels: {
        "org.opencontainers.image.version": "0.1.0",
        "org.opencontainers.image.revision": SHA,
      },
    },
  };

  test("one key spelled 'linux/amd64,linux/arm64' is not two platforms", () => {
    withFile({ "linux/amd64,linux/arm64": labels }, (path) => {
      expect(refusal(["image-labels", path, "0.1.0", SHA]).code).toBe(1);
    });
  });

  test("a platform entry that is not an object is refused", () => {
    withFile({ "linux/amd64": labels, "linux/arm64": [] }, (path) => {
      expect(refusal(["image-labels", path, "0.1.0", SHA]).code).toBe(1);
    });
  });

  test("invalid UTF-8 in release.json is refused, not replaced", () => {
    const text = JSON.stringify({ ...RECORD, notes: "XX" });
    const bytes = new TextEncoder().encode(text);
    bytes[text.indexOf("XX")] = 0xff;
    withFile(bytes, (path) => {
      for (const argv of [
        ["version", path, "v0.1.0"],
        ["field", path, "indexDigest"],
        ["field", path, "image"],
        ["record-digest", path, "0.1.0", SHA],
      ]) {
        const error = refusal(argv);
        expect(error.code).toBe(1);
        expect(error.message).toContain("is not valid UTF-8");
      }
    });
  });

  test("a UTF-8 BOM in release.json is refused", () => {
    const json = new TextEncoder().encode(JSON.stringify(RECORD));
    withFile(new Uint8Array([0xef, 0xbb, 0xbf, ...json]), (path) => {
      expect(refusal(["field", path, "indexDigest"]).code).toBe(1);
    });
  });

  test("a release state listing non-string assets is refused", () => {
    const state = JSON.stringify({ state: "published", assets: [...RELEASE_ASSETS, {}] });
    expect(refusal(["assets-complete", state, ...RELEASE_ASSETS]).code).toBe(1);
  });
});
