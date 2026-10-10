// Packet producer/consumer controls on a real fixture Git checkout with real
// ELF copies, ldd, ABI files, compiler environment and CI identity checks.
// HostFacts replaces only the input collector, toolchain versions and free disk.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { strict as assert } from "node:assert";
import {
  chmodSync,
  closeSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import { Header } from "tar";
import { browserStages, collaborationStages } from "./admission.ts";
import { abiFiles, buildEnv, reference } from "./build.ts";
import {
  admit,
  browserAfter,
  browserStage,
  buildInputs,
  consume,
  distFiles,
  exportPacket,
  inputDiagnostics,
  members,
} from "./handoff.ts";
import type { HostFacts } from "./handoff.ts";
import { call, digest, sha } from "./io.ts";

let SHA = "",
  TREE = "";
const collaborationNames = [
  "fvoci-server",
  "fvoci-migrate",
  "fvoci-e2e-fixture",
  "fvoci_server",
  "selected_install_lifetime",
  "collab-engine",
];
type Row = Record<string, unknown>;
interface Entry {
  path: string;
  sha256: string;
  bytes: number;
  mode: number;
}
interface Manifest extends Row {
  entries: Record<string, Entry>;
  payload_sha256: string;
}
interface BinaryRecord {
  sha256: string;
  bytes: number;
  target: { name: string; kind?: unknown };
  compiledSource: string;
  targetTriple: string;
  features: string[];
  profile: Row;
}
interface BundleRecord extends Row {
  binaries: Record<string, BinaryRecord>;
  compiler_artifacts: unknown[];
}
interface AbiRecord extends Row {
  actualCurrentELFldd: Record<string, string>;
  host_runtime_files: Record<string, string>;
}
interface Bounded {
  count: number;
  entries: unknown[] | Record<string, unknown>;
  truncated: boolean;
}
interface Consumed extends Row {
  received: Record<string, unknown>;
}

interface Fixture {
  base: string;
  root: string;
  output: string;
  packet: string;
  header: string;
  dist: string;
  target: string;
  free: number;
  host: HostFacts;
}
let f: Fixture;
// Fixture lookups that must exist; a missing one fails the test, not the check.
function first<T>(items: T[]): T {
  const item = items[0];
  assert.ok(item !== undefined, "empty fixture list");
  return item;
}
function at<T>(map: Record<string, T>, key: string): T {
  const item = map[key];
  assert.ok(item !== undefined, "missing fixture key");
  return item;
}
const saved = { ...process.env };

const put = (name: string, value: unknown) => {
  writeFileSync(join(f.output, name), JSON.stringify(value));
};
const get = (name: string): unknown => JSON.parse(readFileSync(join(f.output, name), "utf8"));
// A real ldd receipt with fixed mapping addresses: ASLR differs per run.
const recordedLdd = (path: string) => call(["ldd", path]).replace(/0x[0-9a-f]+/g, "0x1111") + "\n";
const toolchain = () => ({
  rustc: "release: 1.98.1\nhost: x86_64-unknown-linux-gnu",
  cargo: "cargo 1.98.1",
  bun: "1.4.2",
});
const abiRecord = () => Object.fromEntries(abiFiles().map((p) => [p, sha(p)]));
const fixtureInputs = () => ({
  head: SHA,
  tree: TREE,
  status: "",
  tracked: { "fixture-source": sha(f.header) },
  external: { [f.header]: sha(f.header) },
  untracked: {},
});
function setEnvironment(values: Record<string, string>): void {
  for (const key of Object.keys(process.env))
    if (key.startsWith("GITHUB_") || key.startsWith("FVOCI_") || key === "CI")
      Reflect.deleteProperty(process.env, key);
  Object.assign(process.env, values);
}

function collaborationFixture(): void {
  const base = realpathSync(mkdtempSync(join(tmpdir(), "fvoci-handoff-")));
  const root = join(base, "repo"),
    output = join(base, "output"),
    target = join(root, "target");
  mkdirSync(root);
  // A real checkout: identity() and the browser clean-tree check run for real.
  for (const args of [
    ["init", "-q"],
    [
      "-c",
      "user.name=fixture",
      "-c",
      "user.email=fixture@invalid",
      "commit",
      "-q",
      "--allow-empty",
      "-m",
      "fixture",
    ],
  ])
    call(["git", ...args], root);
  SHA = call(["git", "rev-parse", "HEAD"], root);
  TREE = call(["git", "rev-parse", "HEAD^{tree}"], root);
  mkdirSync(output, { mode: 0o700 });
  chmodSync(output, 0o700);
  const header = join(root, "header"),
    dist = join(root, "apps/web/dist");
  writeFileSync(header, "official fixture header");
  mkdirSync(dist, { recursive: true });
  writeFileSync(join(dist, "index.html"), "fresh fixture dist");
  f = {
    base,
    root,
    output,
    packet: join(base, "packet"),
    header,
    dist,
    target,
    free: 20_000_000_000,
    host: undefined as unknown as HostFacts,
  };
  f.host = { checkout: root, inputs: fixtureInputs, toolchain, freeBytes: () => f.free };
  setEnvironment({
    CI: "true",
    GITHUB_ACTIONS: "true",
    GITHUB_JOB: "collaboration-build",
    FVOCI_WEB_BUILD_PHASE: "prepare",
    GITHUB_REPOSITORY: "AISFlow/fvoci",
    GITHUB_RUN_ID: "123",
    GITHUB_RUN_ATTEMPT: "1",
    GITHUB_SHA: SHA,
    FVOCI_SELECTED_CI_OUTPUT: output,
    FVOCI_WEB_BUILD_HANDOFF: f.packet,
    CARGO_TARGET_DIR: target,
    GITHUB_OUTPUT: join(base, "github-output"),
    SQLITE3_LIB_DIR: "/fixed/lib",
    SQLITE3_INCLUDE_DIR: "/fixed/include",
    SQLITE3_STATIC: "1",
    SQLITE3_NO_PKG_CONFIG: "1",
  });
  put("before.json", fixtureInputs());
  put("after.json", fixtureInputs());
  put("build-env-inputs.json", buildEnv());
  writeStages(collaborationStages);
  const bins: Record<string, BinaryRecord> = {};
  mkdirSync(join(target, "debug/deps"), { recursive: true });
  for (const name of collaborationNames) {
    const path = join(target, "debug", name);
    copyFileSync("/usr/bin/true", path);
    chmodSync(path, 0o755);
    bins[path] = {
      sha256: sha(path),
      bytes: statSync(path).size,
      target: { name },
      compiledSource: SHA,
      targetTriple: "x86_64-unknown-linux-gnu",
      features: name === "collab-engine" ? ["default", "worker"] : ["api-schema", "db-tests"],
      profile: { test: ["fvoci_server", "selected_install_lifetime"].includes(name) },
    };
  }
  const core = join(target, "debug/deps/libfvoci_server.rlib");
  writeFileSync(core, "NOT RLIB fixture");
  put("bundle.json", {
    source: SHA,
    tree: TREE,
    full_inputs_unchanged: true,
    binaries: bins,
    compiler_artifacts: [
      {
        target: { name: "fvoci_server" },
        profile: { test: false },
        features: ["api-schema", "db-tests"],
        filenames: [core],
      },
    ],
  });
  put("build-environment.json", {
    rustc: toolchain().rustc,
    cargo: "cargo 1.98.1",
    bun: "1.4.2",
    os_release: readFileSync("/etc/os-release", "utf8"),
    target,
    features: ["api-schema", "db-tests"],
    nativeFeatures: ["worker"],
    profile: "debug",
    devDebug: "0",
    testDebug: "0",
    sqlite: {
      SQLITE3_LIB_DIR: "/fixed/lib",
      SQLITE3_INCLUDE_DIR: "/fixed/include",
      SQLITE3_STATIC: "1",
      SQLITE3_NO_PKG_CONFIG: "1",
    },
  });
  put("web-receipt.json", {
    source: SHA,
    tree: TREE,
    exit_code: 0,
    full_inputs_unchanged: true,
    dist_files: { "index.html": sha(join(dist, "index.html")) },
    servedDist: dist,
  });
  put("abi-receipt.json", {
    currentSource: SHA,
    currentELFDependenciesVerified: true,
    host_runtime_files: abiRecord(),
    actualCurrentELFldd: Object.fromEntries(Object.keys(bins).map((p) => [p, recordedLdd(p)])),
  });
}
function writeStages(names: readonly string[]): void {
  const stages: Row[] = [];
  for (const name of names) {
    put(name + "-stage.json", {
      source: SHA,
      tree: TREE,
      command: ["actual-fixed-fixture", name],
      exit_code: 0,
      seconds: 1,
    });
    writeFileSync(join(f.output, name + "-compiler.jsonl"), "{}\n");
    stages.push({
      ...(get(name + "-stage.json") as Row),
      compilerMessages: reference(join(f.output, name + "-compiler.jsonl")),
    });
  }
  const receipt = existsSync(join(f.output, "compile-receipt.json"))
    ? (get("compile-receipt.json") as Row)
    : { source: SHA, tree: TREE, exit_code: 0, full_inputs_unchanged: true };
  put("compile-receipt.json", { ...receipt, stages });
}
function transfer(consumerJob = "collaboration-flow"): Manifest {
  exportPacket(f.host);
  const manifest = JSON.parse(readFileSync(join(f.packet, "handoff.json"), "utf8")) as Manifest;
  // Remove only this test's produced destinations to emulate an empty consumer.
  for (const entry of Object.values(manifest.entries)) unlinkSync(entry.path);
  Object.assign(process.env, {
    GITHUB_JOB: consumerJob,
    FVOCI_WEB_BUILD_PHASE: "consume",
    FVOCI_WEB_BUILD_HANDOFF_SHA256: sha(join(f.packet, "handoff.json")),
  });
  return manifest;
}
function changeManifest(manifest: Row): void {
  writeFileSync(join(f.packet, "handoff.json"), JSON.stringify(manifest));
  process.env.FVOCI_WEB_BUILD_HANDOFF_SHA256 = sha(join(f.packet, "handoff.json"));
}
// Rewrites payload.tar with node-tar headers; `change` may alter any member.
async function rewriteArchive(
  manifest: Manifest,
  change: (member: {
    name: string;
    data: Buffer;
    type: string;
    linkpath?: string;
    mode?: number;
  }) => void,
  extra?: { name: string; type: string },
): Promise<void> {
  const archive = join(f.packet, "payload.tar");
  const listed = await members(archive, () => true);
  const fd = openSync(archive, "w");
  try {
    for (const item of listed) {
      const member = {
        name: item.name,
        data: item.data ?? Buffer.alloc(0),
        type: "File",
        mode: item.mode,
      } as { name: string; data: Buffer; type: string; linkpath?: string; mode?: number };
      change(member);
      const header = Buffer.alloc(512);
      new Header({
        path: member.name,
        mode: member.mode,
        size: member.type === "File" ? member.data.length : 0,
        type: member.type as "File",
        linkpath: member.linkpath,
        mtime: new Date(0),
      }).encode(header, 0);
      writeSync(fd, header);
      if (member.type === "File") {
        writeSync(fd, member.data);
        const padding = (512 - (member.data.length % 512)) % 512;
        if (padding) writeSync(fd, Buffer.alloc(padding));
        const entry = manifest.entries[member.name] as Entry;
        entry.bytes = member.data.length;
        entry.sha256 = digest(member.data);
      }
    }
    if (extra) {
      const header = Buffer.alloc(512);
      new Header({
        path: extra.name,
        mode: 0o600,
        size: 0,
        type: extra.type as "File",
        mtime: new Date(0),
      }).encode(header, 0);
      writeSync(fd, header);
    }
    writeSync(fd, Buffer.alloc(1024));
  } finally {
    closeSync(fd);
  }
  manifest.payload_sha256 = sha(archive);
  changeManifest(manifest);
}

afterEach(() => {
  for (const key of Object.keys(process.env))
    if (!(key in saved)) Reflect.deleteProperty(process.env, key);
  Object.assign(process.env, saved);
  rmSync(f.base, { recursive: true, force: true });
});

describe.serial("collaboration build packet", () => {
  beforeEach(collaborationFixture);

  // Swap one packet binary for another real file and keep bundle facts exact.
  function replaceBinary(source: string | null, text?: string): string {
    const bundle = get("bundle.json") as BundleRecord,
      path = Object.keys(bundle.binaries)[0] as string;
    if (source) copyFileSync(source, path);
    else writeFileSync(path, text ?? "");
    chmodSync(path, 0o755);
    Object.assign(at(bundle.binaries, path), { sha256: sha(path), bytes: statSync(path).size });
    put("bundle.json", bundle);
    return path;
  }
  test("ldd address changes keep the qualified dependency identity", () => {
    const path = Object.keys((get("bundle.json") as BundleRecord).binaries)[0] as string;
    const recorded = (get("abi-receipt.json") as AbiRecord).actualCurrentELFldd[path] as string;
    expect(recorded).not.toBe(call(["ldd", path]) + "\n");
    exportPacket(f.host);
    expect(existsSync(f.packet)).toBe(true);
    expect((get("abi-receipt.json") as AbiRecord).actualCurrentELFldd[path]).toBe(recorded);
    expect(readFileSync(process.env.GITHUB_OUTPUT as string, "utf8")).toBe(
      "handoff_sha256=" + sha(join(f.packet, "handoff.json")) + "\n",
    );
  });
  test("a recorded missing dependency is refused", () => {
    const abi = get("abi-receipt.json") as AbiRecord;
    const key = first(Object.keys(abi.actualCurrentELFldd));
    abi.actualCurrentELFldd[key] =
      "\tlibfixture.so.1 => not found\n" + at(abi.actualCurrentELFldd, key);
    put("abi-receipt.json", abi);
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
    expect(existsSync(f.packet)).toBe(false);
  });
  test("a current dependency set other than the recorded one is refused", () => {
    // bash links more libraries than the recorded true(1) receipt.
    replaceBinary("/usr/bin/bash");
    expect(() => {
      exportPacket(f.host);
    }).toThrow("dependency set differs");
    expect(existsSync(f.packet)).toBe(false);
  });
  test("a matching but unqualified current dependency is refused", () => {
    const path = replaceBinary("/usr/bin/bash");
    const qualified = new Set(abiFiles());
    const outside = call(["ldd", path])
      .split("\n")
      .some(
        (line) =>
          /=> \//.test(line) &&
          !qualified.has(realpathSync(line.split("=> ")[1]?.split(" (")[0] ?? "")),
      );
    expect(outside).toBe(true); // precondition: bash links a library outside the ABI list
    const abi = get("abi-receipt.json") as AbiRecord;
    abi.actualCurrentELFldd[path] = recordedLdd(path);
    put("abi-receipt.json", abi);
    expect(() => {
      exportPacket(f.host);
    }).toThrow("unqualified current ELF dependency");
    expect(existsSync(f.packet)).toBe(false);
  });
  test("recorded host ABI bytes other than the current ones are refused", () => {
    const abi = get("abi-receipt.json") as AbiRecord;
    abi.host_runtime_files[first(Object.keys(abi.host_runtime_files))] = "0".repeat(64);
    put("abi-receipt.json", abi);
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
    expect(existsSync(f.packet)).toBe(false);
  });
  test("a failed current ldd is refused", () => {
    replaceBinary(null, "#!/bin/sh\nexit 0\n"); // not a dynamic executable: ldd exits 1
    expect(() => {
      exportPacket(f.host);
    }).toThrow("ldd failed");
    expect(existsSync(f.packet)).toBe(false);
  });
  test("a recorded receipt missing a dependency is refused", () => {
    const abi = get("abi-receipt.json") as AbiRecord;
    const key = first(Object.keys(abi.actualCurrentELFldd));
    abi.actualCurrentELFldd[key] = "\tlinux-vdso.so.1 (0x1111)\n";
    put("abi-receipt.json", abi);
    expect(() => {
      exportPacket(f.host);
    }).toThrow("dependency set differs");
    expect(existsSync(f.packet)).toBe(false);
  });

  test("a fresh consumer without target directories installs the packet", async () => {
    transfer();
    // A fresh checkout restores only Cargo downloads: no target/ yet.
    rmSync(f.target, { recursive: true, force: true });
    rmSync(join(f.root, "crates/collab-engine/target"), { recursive: true, force: true });
    await consume(f.host);
    expect(Object.keys((get("handoff-consumed.json") as Consumed).received)).toHaveLength(23);
  });
  test("a symlinked missing destination parent is refused", async () => {
    transfer();
    rmSync(f.target, { recursive: true, force: true });
    symlinkSync(join(f.root, "elsewhere"), f.target);
    await assert.rejects(consume(f.host));
    expect(existsSync(join(f.root, "elsewhere"))).toBe(false);
  });
  test("a gzip payload is refused even with a matching digest", async () => {
    const manifest = transfer();
    const archive = join(f.packet, "payload.tar");
    writeFileSync(archive, Bun.gzipSync(readFileSync(archive)));
    manifest.payload_sha256 = sha(archive);
    changeManifest(manifest);
    await assert.rejects(consume(f.host), { message: /not plain tar/ });
    expect(existsSync(join(f.output, "before.json"))).toBe(false);
  });
  test("input fingerprints are the Python json.dumps bytes for non-ASCII keys", () => {
    const before = fixtureInputs();
    const current = { ...before, tracked: { "한글.ts": "x" } };
    inputDiagnostics(f.output, before, before, current);
    const fields = (
      get("handoff-input-current-safe.json") as { fields: Record<string, { sha256: string }> }
    ).fields;
    // python3 -c 'json.dumps({"한글.ts": "x"}, sort_keys=True, separators=(",", ":"))'
    expect(fields.tracked?.sha256).toBe(
      "367d47b24ff50e8c960e1964edea96587de56a2c27e9c76a74b14306d51ea385",
    );
  });
  test("exact current packet is admitted and installed", async () => {
    transfer();
    admit(f.host);
    await consume(f.host);
    const receipt = get("handoff-consumed.json") as Consumed;
    expect(receipt.full_current_physical_inputs_equal).toBe(true);
    expect(receipt.fresh_dist_equal).toBe(true);
    expect(Object.keys(receipt.received)).toHaveLength(23);
    for (const record of Object.values((get("bundle.json") as BundleRecord).binaries))
      expect(record.sha256).toBeString();
    for (const path of Object.keys((get("bundle.json") as BundleRecord).binaries))
      expect(statSync(path).mode & 0o7777).toBe(0o555);
    expect(statSync(join(f.output, "before.json")).mode & 0o7777).toBe(0o600);
  });
  for (const field of [
    "repository",
    "run",
    "attempt",
    "source",
    "tree",
    "root",
    "output",
    "producer_job",
    "consumer_job",
    "schema",
  ])
    test("foreign producer " + field + " refused", () => {
      const manifest = transfer();
      changeManifest({ ...manifest, [field]: "foreign" });
      expect(() => admit(f.host)).toThrow();
      expect(existsSync(join(f.output, "handoff-consumed.json"))).toBe(false);
    });
  test("consumer disk floor refused", () => {
    transfer();
    f.free = 19_999_999_999;
    expect(() => admit(f.host)).toThrow("consumer START disk floor");
  });
  test("missing or wrong manifest digest refused", () => {
    transfer();
    process.env.FVOCI_WEB_BUILD_HANDOFF_SHA256 = "0".repeat(64);
    expect(() => admit(f.host)).toThrow("producer manifest digest differs");
    unlinkSync(join(f.packet, "handoff.json"));
    expect(() => admit(f.host)).toThrow();
  });
  test("failed producer or missing core library refused", () => {
    const original = get("compile-receipt.json") as Row;
    put("compile-receipt.json", { ...original, exit_code: 7 });
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
    expect(existsSync(f.packet)).toBe(false);
    put("compile-receipt.json", original);
    put("bundle.json", { ...(get("bundle.json") as BundleRecord), compiler_artifacts: [] });
    expect(() => {
      exportPacket(f.host);
    }).toThrow("missing emitted core library");
  });
  test("corrupt payload refused", async () => {
    transfer();
    const archive = join(f.packet, "payload.tar");
    writeFileSync(archive, Buffer.concat([readFileSync(archive), Buffer.from("wrong")]));
    await assert.rejects(consume(f.host));
  });
  test("foreign or existing destination refused", async () => {
    const manifest = transfer();
    const entry = first(Object.values(manifest.entries));
    writeFileSync(entry.path, "foreign");
    await assert.rejects(consume(f.host), { message: new RegExp("foreign/existing destination") });
  });
  test("changed current physical input refused", async () => {
    transfer();
    writeFileSync(f.header, "different actual input");
    await assert.rejects(consume(f.host), {
      message: new RegExp("current physical inputs differ"),
    });
    expect(existsSync(join(f.output, "handoff-consumed.json"))).toBe(false);
  });
  test("changed fresh dist refused", async () => {
    transfer();
    writeFileSync(join(f.dist, "index.html"), "different fresh dist");
    await assert.rejects(consume(f.host));
  });
  test("wrong features refused", () => {
    const bundle = get("bundle.json") as BundleRecord;
    first(Object.values(bundle.binaries)).features = [];
    put("bundle.json", bundle);
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
  });
  test("changed compiler environment refused", () => {
    put("build-env-inputs.json", { different: "env" });
    expect(() => {
      exportPacket(f.host);
    }).toThrow("compiler environment differs");
  });
  test("missing receipt refused", () => {
    unlinkSync(join(f.output, "before.json"));
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
  });
  for (const [key, value] of [
    ["GITHUB_JOB", "workspace-browser-shard"],
    ["FVOCI_WEB_BUILD_PHASE", "prepare"],
    ["FVOCI_WEB_BUILD_PHASE", ""],
  ] as const)
    test(`wrong consumer ${key}=${value} refused`, () => {
      transfer();
      process.env[key] = value;
      expect(() => admit(f.host)).toThrow();
    });
  test("symlink manifest refused", () => {
    transfer();
    const manifest = join(f.packet, "handoff.json"),
      actual = join(f.packet, "actual.json");
    renameSync(manifest, actual);
    symlinkSync(actual, manifest);
    expect(() => admit(f.host)).toThrow();
  });
  for (const key of [
    "rustc",
    "cargo",
    "bun",
    "os_release",
    "target",
    "features",
    "nativeFeatures",
    "profile",
    "devDebug",
    "testDebug",
    "sqlite",
  ])
    test("changed build environment " + key + " refused", () => {
      put("build-environment.json", {
        ...(get("build-environment.json") as Row),
        [key]: "foreign",
      });
      expect(() => {
        exportPacket(f.host);
      }).toThrow();
      expect(existsSync(f.packet)).toBe(false);
    });
  test("extra native destination refused before installing anything", async () => {
    const manifest = transfer();
    const entry = { ...first(Object.values(manifest.entries)) };
    entry.path = join(f.target, "debug/foreign-native");
    manifest.entries.extra = entry;
    changeManifest(manifest);
    await assert.rejects(consume(f.host));
    expect(existsSync(join(f.target, "debug/foreign-native"))).toBe(false);
    expect(existsSync(join(f.output, "before.json"))).toBe(false);
  });
  test("changed stage receipt or native bytes refused", () => {
    const original = get("main-stage.json") as Row;
    put("main-stage.json", { ...original, exit_code: 7 });
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
    put("main-stage.json", original);
    writeFileSync(
      Object.keys((get("bundle.json") as BundleRecord).binaries)[0] as string,
      "changed physical ELF",
    );
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
  });
  test("link member refused even with a matching archive digest", async () => {
    const manifest = transfer();
    let first = true;
    await rewriteArchive(manifest, (member) => {
      if (!first) return;
      first = false;
      member.type = "SymbolicLink";
      member.linkpath = "/foreign";
    });
    await assert.rejects(consume(f.host));
    expect(existsSync(join(f.output, "before.json"))).toBe(false);
  });
  test("a raw member mode with file type bits is refused before installing", async () => {
    const manifest = transfer();
    let first = true;
    await rewriteArchive(manifest, (member) => {
      if (!first) return;
      first = false;
      member.mode = 0o100000 | (member.mode ?? 0); // node-tar alone would read 0600
    });
    await assert.rejects(consume(f.host));
    expect(existsSync(join(f.output, "before.json"))).toBe(false);
  });
  test("an unsupported member type outside the manifest is refused", async () => {
    const manifest = transfer();
    await rewriteArchive(manifest, () => undefined, { name: "acl", type: "SolarisACL" });
    await assert.rejects(consume(f.host), { message: /unsupported type/ });
    expect(existsSync(join(f.output, "before.json"))).toBe(false);
  });
  test("input diagnostics are bounded hashes, never paths or values", () => {
    const secret = "https://private.invalid/token-secret";
    const before = { ...fixtureInputs(), status: secret, "unknown-private-field": secret };
    const current = {
      ...before,
      untracked: Object.fromEntries(
        Array.from({ length: 600 }, (_, i) => [`${secret}/${String(i)}`, secret]),
      ),
    };
    put("before.json", before);
    put("after.json", before);
    expect(() => {
      exportPacket({ ...f.host, inputs: () => current });
    }).toThrow("current physical inputs differ");
    const files = readdirSync(f.output)
      .filter((name) => /^handoff-input-.*-safe\.json$/.test(name))
      .sort();
    expect(files).toHaveLength(4);
    for (const name of files) {
      const path = join(f.output, name),
        text = readFileSync(path, "utf8");
      expect(text).not.toContain(secret);
      expect(text).not.toContain("unknown-private-field");
      expect(statSync(path).mode & 0o777).toBe(0o600);
      expect(statSync(path).size).toBeLessThan(256 * 1024);
    }
    const record = (get("handoff-input-current-safe.json") as { fields: { untracked: Bounded } })
      .fields.untracked;
    expect([record.count, Object.keys(record.entries).length, record.truncated]).toEqual([
      600,
      512,
      true,
    ]);
    const delta = (
      get("handoff-input-delta-safe.json") as { after_current: { untracked: Bounded } }
    ).after_current.untracked;
    expect([delta.count, delta.entries.length, delta.truncated]).toEqual([600, 512, true]);
    const savedFiles = files.map((name) => readFileSync(join(f.output, name)));
    expect(() => {
      inputDiagnostics(f.output, before, before, current);
    }).toThrow();
    expect(files.map((name) => readFileSync(join(f.output, name)))).toEqual(savedFiles);
  });
});

function browserFixture(): void {
  collaborationFixture();
  Object.assign(process.env, {
    GITHUB_JOB: "workspace-browser-build",
    FVOCI_WEB_BUILD_PHASE: "prepare",
  });
  for (const name of ["before.json", "after.json"]) put(name, buildInputs(f.host));
  for (const name of collaborationStages) {
    unlinkSync(join(f.output, name + "-stage.json"));
    unlinkSync(join(f.output, name + "-compiler.jsonl"));
  }
  writeStages(browserStages);
  const bundle = get("bundle.json") as BundleRecord;
  bundle.compiler_artifacts = [];
  for (const [path, record] of Object.entries(bundle.binaries)) {
    if (record.profile.test) {
      Reflect.deleteProperty(bundle.binaries, path);
      continue;
    }
    const name = record.target.name;
    record.target.kind = ["bin"];
    record.profile = { test: false, opt_level: "0", debuginfo: 0 };
    record.features =
      name === "collab-engine"
        ? ["default", "worker"]
        : name === "fvoci-e2e-fixture"
          ? ["db-tests"]
          : [];
  }
  put("bundle.json", bundle);
  put("build-environment.json", { ...(get("build-environment.json") as Row), features: [] });
  const abi = get("abi-receipt.json") as AbiRecord;
  abi.actualCurrentELFldd = Object.fromEntries(
    Object.keys(bundle.binaries).map((p) => [p, recordedLdd(p)]),
  );
  put("abi-receipt.json", abi);
}
const browserTransfer = () => transfer("workspace-browser-shard");
const invalidBinaryFields: ["profile" | "target", string, unknown][] = [
  ["profile", "opt_level", "3"],
  ["profile", "opt_level", "missing"],
  ["profile", "opt_level", 0],
  ["profile", "debuginfo", 2],
  ["profile", "debuginfo", "missing"],
  ["profile", "debuginfo", false],
  ["profile", "test", true],
  ["profile", "test", "missing"],
  ["profile", "test", 0],
  ["target", "kind", ["lib"]],
  ["target", "kind", ["bin", "lib"]],
  ["target", "kind", "missing"],
];
function changeBinaryField(
  record: BinaryRecord,
  field: "profile" | "target",
  key: string,
  value: unknown,
): void {
  const part = record[field] as Row;
  if (value === "missing") Reflect.deleteProperty(part, key);
  else part[key] = value;
}
function writeEmittedLogs(bundle: BundleRecord): void {
  for (const stage of browserStages) {
    const lines: string[] = [];
    for (const [path, record] of Object.entries(bundle.binaries)) {
      const name = record.target.name;
      if (
        (stage === "fixture" && name === "fvoci-e2e-fixture") ||
        (stage === "default" && ["fvoci-server", "fvoci-migrate"].includes(name)) ||
        (stage === "engine" && name === "collab-engine")
      )
        lines.push(
          JSON.stringify({
            reason: "compiler-artifact",
            target: record.target,
            executable: path,
            features: record.features,
            profile: record.profile,
          }) + "\n",
        );
    }
    writeFileSync(join(f.output, stage + "-compiler.jsonl"), lines.join(""));
  }
}
const browserError = /browser (target|opt_level|debuginfo|test)/;

describe.serial("browser build packet", () => {
  beforeEach(browserFixture);

  test("a fresh shard without target or dist directories installs the packet", async () => {
    const assets = distFiles(f.host);
    browserTransfer();
    for (const path of [f.target, f.dist, join(f.root, "crates/collab-engine/target")])
      rmSync(path, { recursive: true, force: true });
    await consume(f.host);
    expect(distFiles(f.host)).toEqual(assets);
  });
  test("exact input and asset hashes are equal without a consumer build", async () => {
    const before = buildInputs(f.host),
      assets = distFiles(f.host);
    browserTransfer();
    await consume(f.host);
    expect(buildInputs(f.host)).toEqual(before);
    expect(distFiles(f.host)).toEqual(assets);
    const receipt = get("handoff-consumed.json") as Consumed;
    expect(receipt.fresh_dist_equal).toBe(true);
    expect(Object.keys(receipt.received)).toHaveLength(19);
    expect(receipt.platform).toMatchObject({ system: "Linux", machine: "x86_64" });
    expect(
      Object.fromEntries(
        Object.values((get("bundle.json") as BundleRecord).binaries).map((r) => [
          r.target.name,
          r.features,
        ]),
      ),
    ).toEqual({
      "fvoci-server": [],
      "fvoci-migrate": [],
      "fvoci-e2e-fixture": ["db-tests"],
      "collab-engine": ["default", "worker"],
    });
  });
  for (const name of ["handoff.json", "payload.tar"])
    test("missing " + name + " refused", async () => {
      browserTransfer();
      unlinkSync(join(f.packet, name));
      await assert.rejects(consume(f.host));
      expect(existsSync(join(f.output, "handoff-consumed.json"))).toBe(false);
    });
  for (const key of [
    "source",
    "tree",
    "run",
    "attempt",
    "consumer_job",
    "producer_job",
    "root",
    "output",
    "platform",
  ])
    test("foreign producer " + key + " refused", async () => {
      const manifest = browserTransfer();
      changeManifest({ ...manifest, [key]: "foreign" });
      await assert.rejects(consume(f.host));
      expect(existsSync(join(f.output, "before.json"))).toBe(false);
    });
  test("checkout other than the tested SHA refused", async () => {
    browserTransfer();
    process.env.GITHUB_SHA = "c".repeat(40);
    await assert.rejects(consume(f.host), { message: new RegExp("tested SHA") });
  });
  test("manifest and payload hash mismatch refused before extraction", async () => {
    browserTransfer();
    const expected = process.env.FVOCI_WEB_BUILD_HANDOFF_SHA256;
    process.env.FVOCI_WEB_BUILD_HANDOFF_SHA256 = "0".repeat(64);
    await assert.rejects(consume(f.host), { message: new RegExp("manifest digest") });
    process.env.FVOCI_WEB_BUILD_HANDOFF_SHA256 = expected;
    const archive = join(f.packet, "payload.tar");
    writeFileSync(archive, Buffer.concat([readFileSync(archive), Buffer.from("corrupt")]));
    await assert.rejects(consume(f.host));
    expect(existsSync(join(f.output, "before.json"))).toBe(false);
  });
  for (const fault of ["missing", "hash"])
    test("asset entry " + fault + " refused even with a valid outer hash", async () => {
      const manifest = browserTransfer();
      const asset = Object.keys(manifest.entries).find(
        (name) => at(manifest.entries, name).path === join(f.dist, "index.html"),
      ) as string;
      if (fault === "missing") Reflect.deleteProperty(manifest.entries, asset);
      else at(manifest.entries, asset).sha256 = "0".repeat(64);
      changeManifest(manifest);
      await assert.rejects(consume(f.host));
      expect(existsSync(join(f.output, "before.json"))).toBe(false);
    });
  test("changed physical source refused", async () => {
    browserTransfer();
    writeFileSync(f.header, "drift");
    await assert.rejects(consume(f.host), { message: new RegExp("physical inputs differ") });
    expect(existsSync(join(f.output, "handoff-consumed.json"))).toBe(false);
  });
  test("changed frontend environment refused", async () => {
    browserTransfer();
    process.env.VITE_FIXTURE_INPUT = "drift";
    await assert.rejects(consume(f.host), { message: new RegExp("physical inputs differ") });
    expect(existsSync(join(f.output, "handoff-consumed.json"))).toBe(false);
  });
  test("ignored dotenv input refused", async () => {
    browserTransfer();
    writeFileSync(join(f.root, "apps/web/.env.production.local"), "VITE_FIXTURE=drift");
    await assert.rejects(consume(f.host), { message: new RegExp("physical inputs differ") });
    expect(existsSync(join(f.output, "handoff-consumed.json"))).toBe(false);
  });
  test("schema-feature server cannot replace the default server", () => {
    const bundle = get("bundle.json") as BundleRecord;
    for (const record of Object.values(bundle.binaries))
      if (record.target.name === "fvoci-server") record.features = ["api-schema", "db-tests"];
    put("bundle.json", bundle);
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
    expect(existsSync(f.packet)).toBe(false);
  });
  test("other target triple or host runtime bytes refused", () => {
    const original = get("bundle.json") as BundleRecord,
      changed = structuredClone(original);
    first(Object.values(changed.binaries)).targetTriple = "aarch64-unknown-linux-gnu";
    put("bundle.json", changed);
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
    put("bundle.json", original);
    const abi = get("abi-receipt.json") as AbiRecord;
    abi.host_runtime_files[first(Object.keys(abi.host_runtime_files))] = "0".repeat(64);
    put("abi-receipt.json", abi);
    expect(() => {
      exportPacket(f.host);
    }).toThrow();
  });
  test("existing dist destination and symlink asset refused", async () => {
    browserTransfer();
    writeFileSync(join(f.dist, "index.html"), "stale");
    await assert.rejects(consume(f.host), { message: new RegExp("existing destination") });
    expect(existsSync(join(f.output, "before.json"))).toBe(false);
    unlinkSync(join(f.dist, "index.html"));
    symlinkSync(f.header, join(f.dist, "index.html"));
    await assert.rejects(consume(f.host));
  });
  test("wrong or missing profile and bin kind refused for each recorded binary", () => {
    const original = get("bundle.json") as BundleRecord;
    for (const path of Object.keys(original.binaries))
      for (const [field, key, value] of invalidBinaryFields) {
        const changed = structuredClone(original);
        changeBinaryField(at(changed.binaries, path), field, key, value);
        put("bundle.json", changed);
        expect(() => {
          exportPacket(f.host);
        }).toThrow(browserError);
        expect(existsSync(f.packet)).toBe(false);
      }
  });
  test("wrong or missing emitted profile and bin kind refused for each binary", () => {
    const original = get("bundle.json") as BundleRecord;
    for (const path of Object.keys(original.binaries))
      for (const [field, key, value] of invalidBinaryFields) {
        const changed = structuredClone(original);
        changeBinaryField(at(changed.binaries, path), field, key, value);
        writeEmittedLogs(changed);
        rmSync(join(f.output, "after.json"), { force: true });
        expect(() => {
          browserAfter(f.host);
        }).toThrow(browserError);
        expect(get("bundle.json") as BundleRecord).toEqual(original);
      }
  });
  async function resealBundle(change: (record: BinaryRecord) => void): Promise<void> {
    const manifest = browserTransfer();
    await rewriteArchive(manifest, (member) => {
      if (at(manifest.entries, member.name).path !== join(f.output, "bundle.json")) return;
      const bundle = JSON.parse(member.data.toString("utf8")) as BundleRecord;
      change(first(Object.values(bundle.binaries)));
      member.data = Buffer.from(JSON.stringify(bundle));
    });
  }
  test("consumer rejects a resealed foreign profile", async () => {
    await resealBundle((record) => {
      Object.assign(record.profile, { opt_level: "3", debuginfo: 2 });
    });
    await assert.rejects(consume(f.host), { message: new RegExp("browser opt_level") });
    expect(existsSync(join(f.output, "handoff-consumed.json"))).toBe(false);
  });
  test("consumer rejects a resealed incomplete profile", async () => {
    await resealBundle((record) => {
      record.profile = { test: false };
    });
    await assert.rejects(consume(f.host), { message: new RegExp("browser opt_level") });
  });
  test("consumer rejects a resealed non-bin target", async () => {
    await resealBundle((record) => {
      record.target.kind = ["lib"];
    });
    await assert.rejects(consume(f.host), { message: new RegExp("browser target") });
  });
  test("present null debuginfo is admitted", async () => {
    const bundle = get("bundle.json") as BundleRecord;
    for (const record of Object.values(bundle.binaries)) record.profile.debuginfo = null;
    put("bundle.json", bundle);
    browserTransfer();
    await consume(f.host);
    expect(existsSync(join(f.output, "handoff-consumed.json"))).toBe(true);
  });
  test("producer records actual emitted features and the default stage commands", async () => {
    const bundle = get("bundle.json") as BundleRecord;
    for (const name of [
      "after.json",
      "compile-receipt.json",
      "bundle.json",
      "web-receipt.json",
      "abi-receipt.json",
    ])
      unlinkSync(join(f.output, name));
    writeEmittedLogs(bundle);
    // compile-receipt is rebuilt from the stage receipts already in output.
    browserAfter(f.host);
    exportPacket(f.host);
    expect((get("bundle.json") as BundleRecord).binaries).toEqual(bundle.binaries);
    // Each stage runs its fixed cargo argv; a fixture cargo only exits 0.
    const tools = join(f.base, "bin");
    mkdirSync(tools);
    writeFileSync(join(tools, "cargo"), "#!/bin/sh\nexit 0\n");
    chmodSync(join(tools, "cargo"), 0o755);
    process.env.PATH = tools + ":" + (saved.PATH ?? "");
    const commands: string[][] = [];
    for (const stage of browserStages) {
      for (const suffix of ["-stage.json", "-compiler.jsonl", "-stderr.log"])
        rmSync(join(f.output, stage + suffix), { force: true });
      expect(await browserStage(stage, f.host)).toBe(0);
      commands.push((get(stage + "-stage.json") as { command: string[] }).command);
    }
    const [fixture, server, engine] = commands as [string[], string[], string[]];
    expect(fixture).toContain("--features");
    expect(fixture).toContain("db-tests");
    expect(server).not.toContain("--features");
    expect(server).toContain("fvoci-server");
    expect(server).toContain("fvoci-migrate");
    expect(engine).toContain("worker");
    for (const command of commands)
      expect(command.at(-1)).toBe("--message-format=json-render-diagnostics");
  });
});
