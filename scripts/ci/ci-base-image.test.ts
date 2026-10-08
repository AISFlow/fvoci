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
async function saved(
  layers: Record<string, string | Uint8Array>[],
  config = {},
  metadata = info(),
) {
  const entries: Record<string, string | Uint8Array> = { "config.json": JSON.stringify(config) };
  const names = [];
  for (const [index, layer] of layers.entries()) {
    const name = `${index}/layer.tar`;
    names.push(name);
    entries[name] = await new Bun.Archive(layer).bytes();
  }
  entries["manifest.json"] = JSON.stringify([{ Config: "config.json", Layers: names }]);
  await Bun.write(join(root, "image.tar"), new Bun.Archive(entries));
  await Bun.write(join(root, "info.json"), JSON.stringify(metadata));
  return scanImage(join(root, "image.tar"), join(root, "info.json"), metadata.Architecture);
}

test.each(["amd64", "arm64"])("valid native %s image", async (arch) => {
  await saved([{ "etc/os-release": "ID=ubuntu" }], {}, info(arch));
});
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
test("reject env symlink (not returned by Bun.Archive.files)", async () => {
  const dir = join(root, "source");
  await mkdir(dir);
  await writeFile(join(dir, "target"), "ok");
  await symlink("target", join(dir, ".env"));
  const tar = Bun.spawn(["tar", "-cf", "-", "."], { cwd: dir, stdout: "pipe", stderr: "pipe" });
  const layer = await new Response(tar.stdout).bytes();
  expect(await tar.exited).toBe(0);
  await Bun.write(
    join(root, "image.tar"),
    new Bun.Archive({
      "manifest.json": JSON.stringify([{ Config: "config.json", Layers: ["layer.tar"] }]),
      "config.json": "{}",
      "layer.tar": layer,
    }),
  );
  await Bun.write(join(root, "info.json"), JSON.stringify(info()));
  await expect(
    scanImage(join(root, "image.tar"), join(root, "info.json"), "amd64"),
  ).rejects.toThrow("environment file");
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
