import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { FakeRegistry, type FakeOptions } from "./fake-registry.ts";
import {
  FAKE_GH_TOKEN,
  FAKE_PASSWORD,
  FAKE_USER,
  GITHUB_API,
  IMAGE,
  platformManifest,
  REGISTRY_TOKEN,
  release,
  RELEASE_ASSETS,
} from "./fixtures.ts";
import { refusingClient, type HttpClient, type HttpResponse } from "./http.ts";
import { main } from "./release-api.ts";
import { DOCKER_LIST, DOCKER_MANIFEST, OCI_INDEX, OCI_MANIFEST } from "./registry.ts";

const ENV = {
  REGISTRY_USER: FAKE_USER,
  REGISTRY_PASSWORD: FAKE_PASSWORD,
  GH_TOKEN: FAKE_GH_TOKEN,
  GITHUB_REPOSITORY: "AISFlow/fvoci",
  GITHUB_API_URL: GITHUB_API,
};
const MANIFESTS = "https://ghcr.io/v2/aisflow/fvoci/manifests/";
const BASIC = "Basic " + Buffer.from(`${FAKE_USER}:${FAKE_PASSWORD}`).toString("base64");
const BEARER = `Bearer ${REGISTRY_TOKEN}`;
const sha256 = (text: string) => "sha256:" + createHash("sha256").update(text).digest("hex");

async function run(
  argv: string[],
  http: HttpClient = refusingClient,
  env: Record<string, string | undefined> = ENV,
) {
  const out: string[] = [];
  const err: string[] = [];
  const code = await main(
    argv,
    env,
    http,
    (l) => out.push(l),
    (l) => err.push(l),
  );
  return { code, stdout: out.map((l) => l + "\n").join(""), stderr: err.join("\n") };
}

function seeded(options: FakeOptions = {}, mediaType = OCI_MANIFEST) {
  const fake = new FakeRegistry(options);
  const amd64 = fake.add(mediaType, platformManifest("amd64", mediaType));
  const arm64 = fake.add(mediaType, platformManifest("arm64", mediaType));
  return { fake, amd64, arm64 };
}

/** The index bytes the Python original produces for these two manifests. */
function expectedIndex(media: string, amd64: string, arm64: string, list = OCI_INDEX): string {
  const entry = (arch: string, digest: string) =>
    `{"mediaType":"${media}","digest":"${digest}","size":${String(Buffer.byteLength(platformManifest(arch, media)))},` +
    `"platform":{"architecture":"${arch}","os":"linux"}}`;
  return `{"schemaVersion":2,"mediaType":"${list}","manifests":[${entry("amd64", amd64)},${entry("arm64", arm64)}]}`;
}

async function pushedIndex(options: FakeOptions = {}) {
  const s = seeded(options);
  const pushed = await run(
    ["push-index", "--image", IMAGE, "--amd64", s.amd64, "--arm64", s.arm64],
    s.fake.client,
  );
  expect(pushed.code).toBe(0);
  return { ...s, index: pushed.stdout.trim() };
}

const methods = (fake: FakeRegistry) => fake.requests.map((r) => `${r.method} ${r.url}`);
const status = (code: number): HttpResponse => ({
  status: code,
  headers: {},
  body: new TextEncoder().encode("{}"),
});

describe("network boundary", () => {
  test("the default test client refuses every request", async () => {
    const result = await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"]);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain("network refused in tests: GET https://ghcr.io/v2/");
  });
});

describe("argv", () => {
  test.each([
    [[]],
    [["nope"]],
    [["tag", "--image", IMAGE, "--digest", "sha256:" + "0".repeat(64)]],
    [["registry-digest", "--image", IMAGE, "--tag", "0.1.0", "--extra", "x"]],
    [["registry-digest", "--image", IMAGE, "--tag", "0.1.0", "positional"]],
    [
      [
        "tag",
        "--image",
        IMAGE,
        "--digest",
        "sha256:" + "0".repeat(64),
        "--tag",
        "0.1",
        "--floating=yes",
      ],
    ],
  ])("%j is a usage error before any request", async (argv) => {
    const fake = new FakeRegistry();
    expect((await run(argv, fake.client)).code).toBe(2);
    expect(fake.requests).toEqual([]);
  });

  test("--name=value is accepted", async () => {
    const { fake } = seeded();
    const digest = fake.add(OCI_INDEX, "{}", "0.1.0");
    const result = await run(["registry-digest", `--image=${IMAGE}`, "--tag=0.1.0"], fake.client);
    expect(result).toEqual({ code: 0, stdout: digest + "\n", stderr: "" });
  });
});

describe("registry-digest", () => {
  test("prints the digest of an existing tag through the announced token realm", async () => {
    const fake = new FakeRegistry();
    const digest = fake.add(OCI_INDEX, '{"mediaType":"' + OCI_INDEX + '","manifests":[]}', "0.1.0");
    const result = await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], fake.client);
    expect(result).toEqual({ code: 0, stdout: digest + "\n", stderr: "" });
    expect(fake.requests).toEqual([
      { method: "GET", url: "https://ghcr.io/v2/" },
      {
        method: "GET",
        url: "https://ghcr.io/token?scope=repository%3Aaisflow%2Ffvoci%3Apull&service=ghcr.io",
        authorization: BASIC,
      },
      {
        method: "GET",
        url: MANIFESTS + "0.1.0",
        accept: `${OCI_INDEX}, ${DOCKER_LIST}`,
        authorization: BEARER,
      },
    ]);
  });

  test("prints an empty line for a missing tag", async () => {
    const result = await run(
      ["registry-digest", "--image", IMAGE, "--tag", "0.9.9"],
      new FakeRegistry().client,
    );
    expect(result).toEqual({ code: 0, stdout: "\n", stderr: "" });
  });

  test("anonymous without both credentials", async () => {
    const fake = new FakeRegistry();
    await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], fake.client, {
      REGISTRY_USER: FAKE_USER,
    });
    expect(fake.requests[1]?.authorization).toBeUndefined();
  });

  test("an open registry needs no token", async () => {
    const fake = new FakeRegistry({ auth: "open" });
    fake.add(OCI_INDEX, "{}", "0.1.0");
    expect(
      (await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], fake.client)).code,
    ).toBe(0);
    expect(methods(fake)).toEqual(["GET https://ghcr.io/v2/", `GET ${MANIFESTS}0.1.0`]);
    expect(fake.requests[1]?.authorization).toBeUndefined();
  });

  test.each([401, 403])("token refused with HTTP %i exits 4", async (code) => {
    const fake = new FakeRegistry({
      intercept: (_, path) => (path.startsWith("/token") ? status(code) : undefined),
    });
    const result = await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], fake.client);
    expect(result.code).toBe(4);
    expect(result.stdout).toBe("");
    expect(result.stderr).toBe(
      `release-api: registry token for aisflow/fvoci refused (HTTP ${String(code)})`,
    );
  });

  test.each([401, 403])("manifest refused with HTTP %i exits 4", async (code) => {
    const fake = new FakeRegistry({
      intercept: (_, path) => (path.includes("/manifests/") ? status(code) : undefined),
    });
    const result = await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], fake.client);
    expect(result).toEqual({
      code: 4,
      stdout: "",
      stderr: `release-api: aisflow/fvoci:0.1.0 not readable (HTTP ${String(code)})`,
    });
  });

  test("other registry errors exit 1", async () => {
    const fake = new FakeRegistry({
      intercept: (_, path) => (path.includes("/manifests/") ? status(500) : undefined),
    });
    expect(
      (await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], fake.client)).code,
    ).toBe(1);
    const v2 = new FakeRegistry({
      intercept: (_, path) => (path === "/v2/" ? status(503) : undefined),
    });
    const result = await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], v2.client);
    expect(result).toEqual({
      code: 1,
      stdout: "",
      stderr: "release-api: https://ghcr.io/v2/ answered HTTP 503",
    });
  });

  test("a non-bearer challenge is refused", async () => {
    const fake = new FakeRegistry({
      intercept: (_, path) =>
        path === "/v2/"
          ? {
              status: 401,
              headers: { "www-authenticate": 'Basic realm="x"' },
              body: new Uint8Array(),
            }
          : undefined,
    });
    const result = await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], fake.client);
    expect(result.code).toBe(1);
    expect(result.stderr).toBe(`release-api: unsupported registry challenge: 'Basic realm="x"'`);
  });

  test("a registry digest header that disagrees with the bytes is refused", async () => {
    const fake = new FakeRegistry({
      intercept: (_, path) =>
        path.includes("/manifests/")
          ? {
              status: 200,
              headers: { "docker-content-digest": "sha256:" + "f".repeat(64) },
              body: new TextEncoder().encode("{}"),
            }
          : undefined,
    });
    const result = await run(["registry-digest", "--image", IMAGE, "--tag", "0.1.0"], fake.client);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain("registry digest sha256:ffff");
  });

  test("invalid tag exits 2 and an invalid image exits 1, both before any request", async () => {
    const fake = new FakeRegistry();
    expect(
      (await run(["registry-digest", "--image", IMAGE, "--tag", ".bad"], fake.client)).code,
    ).toBe(2);
    const image = await run(
      ["registry-digest", "--image", "ghcr.io/AISFlow/fvoci:0.1.0", "--tag", "0.1.0"],
      fake.client,
    );
    expect(image.code).toBe(1);
    expect(image.stderr).toBe(
      "release-api: --image 'ghcr.io/AISFlow/fvoci:0.1.0' must be a lowercase registry/repository without tag",
    );
    expect(fake.requests).toEqual([]);
  });
});

describe("push-index", () => {
  test("pushes the index by digest only, with the exact bytes of the original", async () => {
    const { fake, amd64, arm64 } = seeded();
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", amd64, "--arm64", arm64],
      fake.client,
    );
    const bytes = expectedIndex(OCI_MANIFEST, amd64, arm64);
    expect(result).toEqual({ code: 0, stdout: sha256(bytes) + "\n", stderr: "" });
    expect(methods(fake)).toEqual([
      "GET https://ghcr.io/v2/",
      "GET https://ghcr.io/token?scope=repository%3Aaisflow%2Ffvoci%3Apull%2Cpush&service=ghcr.io",
      `GET ${MANIFESTS}${amd64}`,
      `GET ${MANIFESTS}${arm64}`,
      `PUT ${MANIFESTS}${sha256(bytes)}`,
    ]);
    expect(fake.requests[2]?.accept).toBe(`${OCI_MANIFEST}, ${DOCKER_MANIFEST}`);
    expect(fake.requests[4]).toMatchObject({
      contentType: OCI_INDEX,
      bodySha256: sha256(bytes),
      authorization: BEARER,
    });
    expect(fake.tags.size).toBe(0);
  });

  test("two docker manifests make a docker manifest list", async () => {
    const { fake, amd64, arm64 } = seeded({}, DOCKER_MANIFEST);
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", amd64, "--arm64", arm64],
      fake.client,
    );
    expect(result.stdout).toBe(
      sha256(expectedIndex(DOCKER_MANIFEST, amd64, arm64, DOCKER_LIST)) + "\n",
    );
  });

  test("the media type falls back to Content-Type when the body has none", async () => {
    const fake = new FakeRegistry();
    const body = '{"schemaVersion":2}';
    const amd64 = fake.add(DOCKER_MANIFEST, body);
    const arm64 = fake.add(OCI_MANIFEST, platformManifest("arm64"));
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", amd64, "--arm64", arm64],
      fake.client,
    );
    expect(result.code).toBe(0);
    const pushed = fake.manifests.get(result.stdout.trim());
    expect(
      (JSON.parse(new TextDecoder().decode(pushed?.body)) as { manifests: unknown[] }).manifests[0],
    ).toEqual({
      mediaType: DOCKER_MANIFEST,
      digest: amd64,
      size: body.length,
      platform: { architecture: "amd64", os: "linux" },
    });
  });

  test("index entry size is the manifest's byte length, not its character count", async () => {
    const fake = new FakeRegistry();
    const text = `{"mediaType":"${OCI_MANIFEST}","annotations":{"title":"한글"}}`;
    const amd64 = fake.add(OCI_MANIFEST, text);
    const arm64 = fake.add(OCI_MANIFEST, platformManifest("arm64"));
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", amd64, "--arm64", arm64],
      fake.client,
    );
    const pushed = fake.manifests.get(result.stdout.trim());
    const entries = (
      JSON.parse(new TextDecoder().decode(pushed?.body)) as { manifests: { size: number }[] }
    ).manifests;
    expect(entries[0]?.size).toBe(Buffer.byteLength(text));
    expect(entries[0]?.size).not.toBe(text.length);
  });

  test("a missing platform manifest is refused without a push", async () => {
    const { fake, amd64 } = seeded();
    const missing = "sha256:" + "a".repeat(64);
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", amd64, "--arm64", missing],
      fake.client,
    );
    expect(result).toEqual({
      code: 1,
      stdout: "",
      stderr: `release-api: ${IMAGE}@${missing} (arm64) does not exist`,
    });
    expect(fake.requests.some((r) => r.method === "PUT")).toBe(false);
  });

  test("an index in place of a platform manifest is refused", async () => {
    const { fake, amd64 } = seeded();
    const nested = fake.add(OCI_INDEX, `{"mediaType":"${OCI_INDEX}","manifests":[]}`);
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", amd64, "--arm64", nested],
      fake.client,
    );
    expect(result.code).toBe(1);
    expect(result.stderr).toBe(
      `release-api: ${IMAGE}@${nested} (arm64) is '${OCI_INDEX}', not a single-platform image manifest`,
    );
    expect(fake.requests.some((r) => r.method === "PUT")).toBe(false);
  });

  test("content that does not hash to the requested digest is refused", async () => {
    const { fake, amd64, arm64 } = seeded();
    const stored = fake.manifests.get(amd64);
    if (stored) fake.manifests.set(amd64, { ...stored, body: new TextEncoder().encode("{}") });
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", amd64, "--arm64", arm64],
      fake.client,
    );
    expect(result.code).toBe(1);
    expect(fake.requests.some((r) => r.method === "PUT")).toBe(false);
  });

  test("an invalid digest is a usage error", async () => {
    const fake = new FakeRegistry();
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", "sha256:abc", "--arm64", "x"],
      fake.client,
    );
    expect(result).toEqual({
      code: 2,
      stdout: "",
      stderr: "release-api: --amd64 must be sha256:<64 hex>",
    });
  });

  test.each([403, 500])("a refused push (HTTP %i) exits 1, not 4", async (code) => {
    const s = seeded({ intercept: (r) => (r.method === "PUT" ? status(code) : undefined) });
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", s.amd64, "--arm64", s.arm64],
      s.fake.client,
    );
    expect(result.code).toBe(1);
    expect(result.stdout).toBe("");
    expect(result.stderr).toMatch(
      new RegExp(
        `^release-api: push aisflow/fvoci:sha256:[0-9a-f]{64}: HTTP ${String(code)} '\\{\\}'$`,
      ),
    );
  });

  test("a push answered with 200 instead of 201 is refused", async () => {
    const s = seeded({ intercept: (r) => (r.method === "PUT" ? status(200) : undefined) });
    const result = await run(
      ["push-index", "--image", IMAGE, "--amd64", s.amd64, "--arm64", s.arm64],
      s.fake.client,
    );
    expect(result.code).toBe(1);
    expect(result.stderr).toContain(": HTTP 200, digest None != sha256:");
  });
});

describe("describe", () => {
  test("prints the index and per-platform digests", async () => {
    const { fake, amd64, arm64, index } = await pushedIndex();
    fake.requests.length = 0;
    const result = await run(["describe", "--image", IMAGE, "--digest", index], fake.client);
    expect(result).toEqual({
      code: 0,
      stdout: `index_digest=${index}\namd64_digest=${amd64}\narm64_digest=${arm64}\n`,
      stderr: "",
    });
    expect(methods(fake)[1]).toBe(
      "GET https://ghcr.io/token?scope=repository%3Aaisflow%2Ffvoci%3Apull&service=ghcr.io",
    );
  });

  const index = (platforms: [string, string][]) =>
    JSON.stringify({
      schemaVersion: 2,
      mediaType: OCI_INDEX,
      manifests: platforms.map(([os, architecture], i) => ({
        mediaType: OCI_MANIFEST,
        digest: "sha256:" + String(i).repeat(64),
        size: 1,
        platform: { os, architecture },
      })),
    });

  test.each([
    [[["linux", "amd64"]], "platforms ['linux/amd64'] are not linux/amd64 and linux/arm64"],
    [
      [
        ["linux", "amd64"],
        ["linux", "amd64"],
      ],
      "unexpected or repeated platform linux/amd64",
    ],
    [
      [
        ["linux", "amd64"],
        ["linux", "arm64"],
        ["linux", "s390x"],
      ],
      "unexpected or repeated platform linux/s390x",
    ],
  ])("refuses %j", async (platforms, message) => {
    const fake = new FakeRegistry();
    const digest = fake.add(OCI_INDEX, index(platforms as [string, string][]));
    const result = await run(["describe", "--image", IMAGE, "--digest", digest], fake.client);
    expect(result).toEqual({
      code: 1,
      stdout: "",
      stderr: `release-api: ${IMAGE}@${digest}: ${message}`,
    });
  });

  test("refuses a platform manifest and a missing digest", async () => {
    const { fake, amd64 } = seeded();
    const single = await run(["describe", "--image", IMAGE, "--digest", amd64], fake.client);
    expect(single.stderr).toBe(`release-api: ${IMAGE}@${amd64} is '${OCI_MANIFEST}', not an index`);
    const missing = "sha256:" + "b".repeat(64);
    const absent = await run(["describe", "--image", IMAGE, "--digest", missing], fake.client);
    expect(absent).toEqual({
      code: 1,
      stdout: "",
      stderr: `release-api: ${IMAGE}@${missing} does not exist`,
    });
    expect((await run(["describe", "--image", IMAGE, "--digest", "0.1.0"], fake.client)).code).toBe(
      2,
    );
  });
});

describe("tag", () => {
  test("tags an untagged index and reads it back", async () => {
    const { fake, index } = await pushedIndex();
    fake.requests.length = 0;
    const result = await run(
      ["tag", "--image", IMAGE, "--digest", index, "--tag", "0.1.0"],
      fake.client,
    );
    expect(result).toEqual({ code: 0, stdout: "", stderr: `${IMAGE}:0.1.0 -> ${index}` });
    expect(methods(fake).slice(2)).toEqual([
      `GET ${MANIFESTS}${index}`,
      `GET ${MANIFESTS}0.1.0`,
      `PUT ${MANIFESTS}0.1.0`,
      `GET ${MANIFESTS}0.1.0`,
    ]);
    expect(fake.requests[4]).toMatchObject({ contentType: OCI_INDEX, bodySha256: index });
    expect(fake.tags.get("0.1.0")).toBe(index);
  });

  test("an already matching tag is a no-op without a push", async () => {
    const { fake, index } = await pushedIndex();
    fake.tags.set("0.1.0", index);
    fake.requests.length = 0;
    const result = await run(
      ["tag", "--image", IMAGE, "--digest", index, "--tag", "0.1.0"],
      fake.client,
    );
    expect(result).toEqual({ code: 0, stdout: "", stderr: `${IMAGE}:0.1.0 already ${index}` });
    expect(fake.requests.some((r) => r.method === "PUT")).toBe(false);
  });

  test("never moves an existing immutable tag to another digest", async () => {
    const { fake, index } = await pushedIndex();
    const other = fake.add(OCI_INDEX, `{"mediaType":"${OCI_INDEX}","manifests":[]}`, "0.1.0");
    fake.requests.length = 0;
    const result = await run(
      ["tag", "--image", IMAGE, "--digest", index, "--tag", "0.1.0"],
      fake.client,
    );
    expect(result).toEqual({
      code: 1,
      stdout: "",
      stderr: `release-api: ${IMAGE}:0.1.0 is ${other}, not ${index}; immutable tags are never moved`,
    });
    expect(fake.requests.some((r) => r.method === "PUT")).toBe(false);
    expect(fake.tags.get("0.1.0")).toBe(other);
  });

  test("--floating moves an existing tag", async () => {
    const { fake, index } = await pushedIndex();
    const other = fake.add(OCI_INDEX, `{"mediaType":"${OCI_INDEX}","manifests":[]}`, "0.1");
    const result = await run(
      ["tag", "--image", IMAGE, "--digest", index, "--tag", "0.1", "--floating"],
      fake.client,
    );
    expect(result).toEqual({
      code: 0,
      stdout: "",
      stderr: `${IMAGE}:0.1 -> ${index} (was ${other})`,
    });
    expect(fake.tags.get("0.1")).toBe(index);
  });

  test("a read-back that names another digest fails", async () => {
    let pushed = false;
    const { fake, index } = await pushedIndex({
      intercept: (r, path) => {
        if (r.method === "PUT" && path.endsWith("/0.1.0")) pushed = true;
        else if (pushed && path.endsWith("/0.1.0")) return status(404);
        return undefined;
      },
    });
    const result = await run(
      ["tag", "--image", IMAGE, "--digest", index, "--tag", "0.1.0"],
      fake.client,
    );
    expect(result).toEqual({
      code: 1,
      stdout: "",
      stderr: `release-api: ${IMAGE}:0.1.0 reads back as None, not ${index}`,
    });
  });

  test("partial failure: the index stays pushed by digest when tagging is refused", async () => {
    const { fake, index } = await pushedIndex({
      intercept: (r, path) =>
        r.method === "PUT" && !path.includes("sha256:") ? status(403) : undefined,
    });
    expect(fake.manifests.has(index)).toBe(true);
    const result = await run(
      ["tag", "--image", IMAGE, "--digest", index, "--tag", "0.1.0"],
      fake.client,
    );
    expect(result.code).toBe(1);
    expect(result.stderr).toStartWith("release-api: push aisflow/fvoci:0.1.0: HTTP 403");
    expect(fake.tags.has("0.1.0")).toBe(false);
    expect(fake.manifests.has(index)).toBe(true);
  });

  test("refusals before any write", async () => {
    const fake = new FakeRegistry();
    const missing = "sha256:" + "c".repeat(64);
    expect(
      (await run(["tag", "--image", IMAGE, "--digest", missing, "--tag", "0.1.0"], fake.client))
        .stderr,
    ).toBe(`release-api: ${IMAGE}@${missing} does not exist`);
    expect(
      (await run(["tag", "--image", IMAGE, "--digest", missing, "--tag", "-x"], fake.client)).code,
    ).toBe(2);
    expect(
      (await run(["tag", "--image", IMAGE, "--digest", "latest", "--tag", "0.1.0"], fake.client))
        .code,
    ).toBe(2);
    expect(fake.requests.some((r) => r.method === "PUT")).toBe(false);
    const denied = new FakeRegistry({
      intercept: (_, path) => (path.startsWith("/token") ? status(401) : undefined),
    });
    expect(
      (await run(["tag", "--image", IMAGE, "--digest", missing, "--tag", "0.1.0"], denied.client))
        .code,
    ).toBe(4);
  });
});

describe("release-state", () => {
  const state = (pages: unknown[][], env: Record<string, string | undefined> = ENV) => {
    const fake = new FakeRegistry({ releasePages: pages });
    return run(["release-state", "--tag", "v0.1.0"], fake.client, env).then((r) => ({
      ...r,
      fake,
    }));
  };
  const filler = (n: number, from = 10) =>
    Array.from({ length: n }, (_, i) => release(`v0.0.${String(from + i)}`, false, [], from + i));

  test("none", async () => {
    const result = await state([[release("v0.0.9", false, [])]]);
    expect(result.stdout).toBe('{"state": "none", "assets": []}\n');
    expect(result.fake.requests).toEqual([
      {
        method: "GET",
        url: `${GITHUB_API}/repos/AISFlow/fvoci/releases?per_page=100&page=1`,
        accept: "application/vnd.github+json",
        authorization: `Bearer ${FAKE_GH_TOKEN}`,
      },
    ]);
  });

  test("published with sorted assets, draft, and ASCII-escaped names", async () => {
    const published = await state([[release("v0.1.0", false, [...RELEASE_ASSETS].reverse())]]);
    expect(published.stdout).toBe(
      '{"state": "published", "assets": ["INSTALL.md", "RELEASE-NOTES.md", "SHA256SUMS", "compose.yml", "env.example", "release.json"]}\n',
    );
    const draft = await state([[release("v0.1.0", true, ["노트.md", "del\u007f"])]]);
    expect(draft.stdout).toBe(
      '{"state": "draft", "assets": ["del\\u007f", "\\ub178\\ud2b8.md"]}\n',
    );
  });

  test("pages until a short page and finds a release on page two", async () => {
    const result = await state([filler(100), [release("v0.1.0", false, ["a"], 7)]]);
    expect(result.stdout).toBe('{"state": "published", "assets": ["a"]}\n');
    expect(result.fake.requests.map((r) => r.url.slice(-6))).toEqual(["page=1", "page=2"]);
  });

  test("two releases naming the tag are refused", async () => {
    const result = await state([[release("v0.1.0", false, []), release("v0.1.0", true, [], 2)]]);
    expect(result).toMatchObject({
      code: 1,
      stdout: "",
      stderr: "release-api: 2 releases name v0.1.0; delete the extra ones by hand",
    });
  });

  test("more than 50 full pages is refused", async () => {
    const result = await state(Array.from({ length: 51 }, () => filler(100)));
    expect(result.code).toBe(1);
    expect(result.stderr).toBe("release-api: more than 5000 releases; refusing to guess");
    expect(result.fake.requests).toHaveLength(50);
  });

  test("a refused listing exits 1", async () => {
    const result = await state([[]], { ...ENV, GH_TOKEN: "wrong" });
    expect(result).toMatchObject({
      code: 1,
      stderr: "release-api: listing releases of AISFlow/fvoci: HTTP 401",
    });
  });

  test("missing environment is a usage error before any request", async () => {
    for (const env of [
      { ...ENV, GH_TOKEN: "" },
      { ...ENV, GITHUB_REPOSITORY: "fvoci" },
    ]) {
      const result = await state([[]], env);
      expect(result.code).toBe(2);
      expect(result.fake.requests).toEqual([]);
    }
  });

  test("a trailing slash on GITHUB_API_URL is dropped", async () => {
    const result = await state([[]], { ...ENV, GITHUB_API_URL: GITHUB_API + "//" });
    expect(result.fake.requests[0]?.url).toBe(
      `${GITHUB_API}/repos/AISFlow/fvoci/releases?per_page=100&page=1`,
    );
  });
});
