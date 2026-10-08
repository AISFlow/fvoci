import { afterEach, beforeEach, expect, test } from "bun:test";
import { mkdtemp, rm, mkdir, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  archDigest,
  buildSummary,
  scanImage,
  verifyManifest,
  verifyMetadata,
  verifyWorkflow,
} from "./ci-base-image";

let root: string;
beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), "fvoci-ci-base-test-"));
});
afterEach(async () => {
  await rm(root, { recursive: true });
});
const info = (arch = "amd64") => ({
  Architecture: arch,
  Os: "linux",
  Size: 123,
  Config: {
    User: "1000:1000",
    Env: ["PATH=/usr/bin"],
    Labels: { "org.opencontainers.image.source": "https://github.com/AISFlow/fvoci" },
  },
});
type Layer = Record<string, string | Uint8Array> | Uint8Array;
const compressions = ["gzip", "zstd"] as const;
const formats = ["tar", ...compressions] as const;
function concat(...parts: Uint8Array[]) {
  const bytes = new Uint8Array(parts.reduce((size, part) => size + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.length;
  }
  return bytes;
}
async function compressed(layer: Layer, format: (typeof compressions)[number]) {
  const bytes = layer instanceof Uint8Array ? layer : await new Bun.Archive(layer).bytes();
  return format === "gzip" ? Bun.gzipSync(bytes) : Bun.zstdCompressSync(bytes);
}
async function encoded(layer: Layer, format: (typeof formats)[number]) {
  return format === "tar"
    ? layer instanceof Uint8Array
      ? layer
      : new Bun.Archive(layer).bytes()
    : compressed(layer, format);
}
async function envSymlinkLayer() {
  const dir = join(root, "source");
  await mkdir(dir);
  await writeFile(join(dir, "target"), "ok");
  await symlink("target", join(dir, ".env"));
  const tar = Bun.spawn(["tar", "-cf", "-", "."], { cwd: dir, stdout: "pipe", stderr: "pipe" });
  const [layer] = await Promise.all([
    new Response(tar.stdout).bytes(),
    new Response(tar.stderr).text(),
  ]);
  expect(await tar.exited).toBe(0);
  return layer;
}
async function saved(layers: Layer[], config = {}, metadata = info(), layerNames?: string[]) {
  const entries: Record<string, string | Uint8Array> = { "config.json": JSON.stringify(config) };
  const names = [];
  for (const [index, layer] of layers.entries()) {
    const name = layerNames?.[index] ?? `${index}/layer.tar`;
    names.push(name);
    entries[name] = layer instanceof Uint8Array ? layer : await new Bun.Archive(layer).bytes();
  }
  entries["manifest.json"] = JSON.stringify([{ Config: "config.json", Layers: names }]);
  await Bun.write(join(root, "image.tar"), new Bun.Archive(entries));
  await Bun.write(join(root, "info.json"), JSON.stringify(metadata));
  return scanImage(join(root, "image.tar"), join(root, "info.json"), metadata.Architecture);
}

test.each(["amd64", "arm64"])("valid native %s image", async (arch) => {
  await saved([{ "etc/os-release": "ID=ubuntu" }], {}, info(arch));
});
test.each(compressions)("valid %s-compressed saved layer", async (format) => {
  await saved([await compressed({ "etc/os-release": "ID=ubuntu" }, format)]);
});
test.each(compressions)("reject truncated %s layer", async (format) => {
  const bytes = await compressed({ ok: "ok" }, format);
  await expect(saved([bytes.subarray(0, 8)])).rejects.toThrow("layer decompression failed");
});
test.each(compressions)("reject trailing garbage after %s layer", async (format) => {
  const bytes = await compressed({ ok: "ok" }, format);
  const corrupt = new Uint8Array(bytes.length + 4);
  corrupt.set(bytes);
  corrupt.set([1, 2, 3, 4], bytes.length);
  await expect(saved([corrupt])).rejects.toThrow("layer decompression failed");
});
test.each(compressions)(
  "reject NUL padding and credential tails after %s layer",
  async (format) => {
    const secret = "ghp_" + "a".repeat(36);
    const layer = await compressed({ ok: "ok" }, format);
    for (const tail of [
      new Uint8Array([0]),
      new Uint8Array([0, 1, 2, 3, 4]),
      new TextEncoder().encode("\0" + secret),
    ]) {
      const error = await saved([concat(layer, tail)]).catch((error) => error);
      expect(error).toBeInstanceOf(Error);
      expect(error.message).toContain("layer decompression failed");
      expect(error.message).not.toContain(secret);
    }
  },
);
test.each(formats)("reject env symlink after EOF in concatenated %s archives", async (format) => {
  const first = await encoded({ ok: "ok" }, format);
  const second = await encoded(await envSymlinkLayer(), format);
  await expect(saved([concat(first, second)])).rejects.toThrow("environment file");
});
test.each(formats)("scan valid concatenated %s archives", async (format) => {
  await saved([
    concat(await encoded({ first: "ok" }, format), await encoded({ second: "ok" }, format)),
  ]);
});
test.each(formats)("reject credential after EOF in concatenated %s archives", async (format) => {
  const first = await encoded({ ok: "ok" }, format);
  const second = await encoded({ token: "ghp_" + "a".repeat(36) }, format);
  await expect(saved([concat(first, second)])).rejects.toThrow("credential/DB URL pattern found");
});
test.each(formats)("reject malformed decoded tail after EOF in %s layer", async (format) => {
  const bytes = await new Bun.Archive({ ok: "ok" }).bytes();
  for (const tail of [new TextEncoder().encode("bad tail"), new Uint8Array(512).fill(1)]) {
    await expect(saved([await encoded(concat(bytes, tail), format)])).rejects.toThrow(
      "layer archive listing failed",
    );
  }
});
test.each(compressions)("reject corrupt %s layer checksum/frame", async (format) => {
  const bytes = await compressed({ ok: "ok" }, format);
  bytes[bytes.length - 1] ^= 0xff;
  await expect(saved([bytes])).rejects.toThrow("layer decompression failed");
});
test("unknown layer format fails with bounded safe diagnostics", async () => {
  const layerName = "blobs/sha256/" + "b".repeat(64);
  const error = await saved([new Uint8Array([1, 2, 3, 4])], {}, info(), [layerName]).catch(
    (error) => error,
  );
  expect(error).toBeInstanceOf(Error);
  expect(error.message).toContain("layer archive listing failed");
  expect(error.message).toContain(`layer 0 (${layerName})`);
  expect(error.message).toContain("magic=01020304");
  expect(error.message).toContain("stderr prefix:");
  expect(error.message).toContain("tar: This does not look like a tar archive");
  expect(error.message.length).toBeLessThan(1024);
});
test("layer identity never echoes an untrusted credential-bearing path", async () => {
  const secret = "ghp_" + "a".repeat(36);
  const error = await saved([new Uint8Array([1, 2, 3, 4])], {}, info(), [secret]).catch(
    (error) => error,
  );
  expect(error).toBeInstanceOf(Error);
  expect(error.message).toContain("layer 0");
  expect(error.message).toContain("name withheld");
  expect(error.message).not.toContain(secret);
});
test.each(["tar", ...compressions] as const)(
  "withhold secret-bearing tar stderr from a broken %s layer",
  async (format) => {
    const secret = "ghp_" + "a".repeat(36);
    await writeFile(join(root, "ok"), "ok");
    const tar = Bun.spawn(
      [
        "tar",
        "--format=pax",
        "--blocking-factor=1",
        `--pax-option=${secret}:=value`,
        "-cf",
        "-",
        "ok",
      ],
      { cwd: root, stdout: "pipe", stderr: "pipe" },
    );
    const [bytes] = await Promise.all([
      new Response(tar.stdout).bytes(),
      new Response(tar.stderr).text(),
    ]);
    expect(await tar.exited).toBe(0);
    const broken = bytes.subarray(0, bytes.length - 1536);
    const listing = Bun.spawn(["tar", "-tf", "-"], {
      stdin: broken,
      stdout: "pipe",
      stderr: "pipe",
      env: { ...process.env, LC_ALL: "C", TAR_OPTIONS: "" },
    });
    const [, stderr] = await Promise.all([
      new Response(listing.stdout).text(),
      new Response(listing.stderr).text(),
    ]);
    expect(await listing.exited).not.toBe(0);
    expect(stderr).toContain(secret); // Prove that GNU tar itself emits the fixture credential.
    const error = await saved([format === "tar" ? broken : await compressed(broken, format)]).catch(
      (error) => error,
    );
    expect(error).toBeInstanceOf(Error);
    expect(error.message).toContain("layer archive listing failed");
    expect(error.message).toContain("stderr prefix:");
    expect(error.message).toContain("content withheld");
    expect(error.message).toContain("tar: Unexpected EOF in archive");
    expect(error.message).not.toContain(secret);
    expect(error.message.length).toBeLessThan(1024);
  },
);
test("reject mismatched architecture", () => {
  expect(() => verifyMetadata(info(), "arm64")).toThrow("native Linux");
});
test("reject root default", () => {
  const metadata = info();
  metadata.Config.User = "0:0";
  expect(() => verifyMetadata(metadata, "amd64")).toThrow("1000:1000");
});
test("reject missing source label", () => {
  const metadata = info();
  metadata.Config.Labels["org.opencontainers.image.source"] = "";
  expect(() => verifyMetadata(metadata, "amd64")).toThrow("source label");
});
test("reject sensitive env key", () => {
  const metadata = info();
  metadata.Config.Env.push("GITHUB_TOKEN=arbitrary-value");
  expect(() => verifyMetadata(metadata, "amd64")).toThrow("environment key");
});
test("reject DB configuration in history", async () => {
  await expect(
    saved([{ ok: "ok" }], { history: [{ created_by: "ENV URL=postgresql://host/db" }] }),
  ).rejects.toThrow("content withheld");
});
test("reject deleted lower-layer credential", async () => {
  await expect(
    saved([{ "tmp/token": "ghp_" + "a".repeat(36) }, { "tmp/.wh.token": "" }]),
  ).rejects.toThrow("content withheld");
});
test.each(compressions)("reject deleted credential in a %s lower layer", async (format) => {
  await expect(
    saved([
      await compressed({ "tmp/token": "ghp_" + "a".repeat(36) }, format),
      await compressed({ "tmp/.wh.token": "" }, format),
    ]),
  ).rejects.toThrow("content withheld");
});
test.each(compressions)("reject DB config with %s layers", async (format) => {
  await expect(
    saved([await compressed({ ok: "ok" }, format)], {
      history: [{ created_by: "ENV URL=postgresql://host/db" }],
    }),
  ).rejects.toThrow("content withheld");
});
test.each(compressions)("reject env file in a %s layer", async (format) => {
  await expect(
    saved([await compressed({ "home/ci/.env.production": "KEY=x" }, format)]),
  ).rejects.toThrow("environment file");
});
test.each(compressions)("reject DB URL in %s text", async (format) => {
  await expect(
    saved([await compressed({ "etc/app.conf": "URL=redis://private-host:6379" }, format)]),
  ).rejects.toThrow("content withheld");
});
test.each(["root/.env", "home/ci/.env.production"])("reject env file %s", async (name) => {
  await expect(saved([{ [name]: "KEY=x" }])).rejects.toThrow("environment file");
});
test("reject DB URL in text", async () => {
  await expect(saved([{ "etc/app.conf": "URL=redis://private-host:6379" }])).rejects.toThrow(
    "content withheld",
  );
});
test("allow compiled protocol literals", async () => {
  await saved([
    { "usr/local/bin/bun": new TextEncoder().encode("\0postgres://localhost\0redis://localhost") },
  ]);
});
test("reject token in a binary", async () => {
  await expect(
    saved([{ "usr/bin/tool": new TextEncoder().encode("\0ghp_" + "a".repeat(36)) }]),
  ).rejects.toThrow("content withheld");
});
test("reject token across stream boundary", async () => {
  await expect(
    saved([{ "tmp/text": "x".repeat(64 * 1024 - 2) + "ghp_" + "a".repeat(36) }]),
  ).rejects.toThrow("content withheld");
});
test.each(compressions)("scan binary literals and credentials in %s layers", async (format) => {
  await saved([
    await compressed({ tool: new TextEncoder().encode("\0postgres://localhost") }, format),
  ]);
  await expect(
    saved([
      await compressed({ tool: new TextEncoder().encode("\0ghp_" + "a".repeat(36)) }, format),
    ]),
  ).rejects.toThrow("content withheld");
});
test.each(compressions)("reject token across a %s stream boundary", async (format) => {
  await expect(
    saved([
      await compressed({ text: "x".repeat(64 * 1024 - 2) + "ghp_" + "a".repeat(36) }, format),
    ]),
  ).rejects.toThrow("content withheld");
});
test.each(formats)("reject env symlink in %s layer", async (format) => {
  await expect(saved([await encoded(await envSymlinkLayer(), format)])).rejects.toThrow(
    "environment file",
  );
});
test("reject missing saved layers", async () => {
  await expect(saved([])).rejects.toThrow("saved image");
});
test("summary includes apt timing, cache and actual size", () => {
  const result = buildSummary(
    "#1 [system-tools 1/1] RUN apt-get update\n#1 DONE 12.3s\n#2 [rust-tools 1/1] RUN rustup\n#2 CACHED\n",
    info(),
  );
  expect(result).toContain("apt-get update | 12.3");
  expect(result).toContain("rustup | cached");
  expect(result).toContain("123 bytes");
});

const workflow: any = Bun.YAML.parse(
  await Bun.file(new URL("../../.github/workflows/ci-base-image.yml", import.meta.url)).text(),
);
test("current workflow obeys publication policy", () => {
  verifyWorkflow(workflow);
});
const mutations: [string, (data: any) => void][] = [
  [
    "default write",
    (d) => {
      d.permissions.packages = "write";
    },
  ],
  [
    "PR write",
    (d) => {
      d.jobs.build.permissions = { packages: "write" };
    },
  ],
  [
    "unguarded push",
    (d) => {
      d.jobs.push.if = "always()";
    },
  ],
  [
    "unguarded manifest",
    (d) => {
      d.jobs["push-manifest"].if = "always()";
    },
  ],
  [
    "contents write",
    (d) => {
      d.jobs["push-manifest"].permissions.contents = "write";
    },
  ],
  [
    "any branch",
    (d) => {
      d.on.push.branches = ["*"];
    },
  ],
  [
    "unrelated filter",
    (d) => {
      d.on.pull_request.paths = ["other/**"];
    },
  ],
  [
    "only x64",
    (d) => {
      d.jobs.build.strategy.matrix.arch = ["amd64"];
    },
  ],
  [
    "new action",
    (d) => {
      d.jobs.build.steps.push({ uses: "docker/setup-qemu-action@v3" });
    },
  ],
  [
    "ignore failure",
    (d) => {
      d.jobs.build.steps.push({ run: "true", "continue-on-error": true });
    },
  ],
  [
    "inline token",
    (d) => {
      d.jobs.build.steps.push({ run: "echo '${{ github.token }}'" });
    },
  ],
  [
    "secrets token",
    (d) => {
      d.jobs.build.steps[2].env.GHCR_TOKEN = "${{ secrets.GITHUB_TOKEN }}";
    },
  ],
  [
    "skip native build",
    (d) => {
      d.jobs.build.steps[3].if = "false";
    },
  ],
  [
    "inline Python",
    (d) => {
      d.jobs.build.steps.push({ run: "python3 -c 'print(1)'" });
    },
  ],
  [
    "gha cache",
    (d) => {
      d.jobs.build.steps.push({ run: "docker buildx build --cache-to type=gha ." });
    },
  ],
];
test.each(mutations)("reject workflow mutation: %s", (_name, mutate) => {
  const data = structuredClone(workflow);
  mutate(data);
  expect(() => verifyWorkflow(data)).toThrow();
});
const digest = "sha256:" + "a".repeat(64),
  second = "sha256:" + "b".repeat(64);
const index = () => ({
  digest,
  manifests: [
    { digest, platform: { os: "linux", architecture: "amd64" } },
    { digest: second, platform: { os: "linux", architecture: "arm64" } },
  ],
});
test("manifest accepts both verified digests", () => {
  verifyManifest(index(), ["image@" + digest, "image@" + second]);
});
test("manifest rejects wrong digest with right platforms", () => {
  expect(() => verifyManifest(index(), ["image@" + digest, "image@" + digest])).toThrow(
    "source digests",
  );
});
test("manifest rejects missing ARM", () => {
  const value = index();
  value.manifests.pop();
  expect(() => verifyManifest(value, [])).toThrow("multi-arch");
});
test("per-arch digest rejects wrong image architecture", () => {
  expect(() =>
    archDigest(
      { architecture: "amd64", os: "linux", config: { User: "1000:1000" } },
      { digest },
      "arm64",
    ),
  ).toThrow("mismatch");
});
