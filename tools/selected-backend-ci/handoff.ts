#!/usr/bin/env bun
// Web build handoff between a producer job and its consumer jobs: the
// collaboration build -> lane jobs, and the browser build -> browser shards.
// Called by scripts/run-web-e2e.sh as `bun handoff.ts <mode>`.
import { deepEquals, spawnSync } from "bun";
import { strict as assert } from "node:assert";
import {
  appendFileSync,
  closeSync,
  createReadStream,
  existsSync,
  fchmodSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  statSync,
  statfsSync,
  writeSync,
} from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import process from "node:process";
import { Header, Parser } from "tar";
import type { ReadEntry } from "tar";
import { browserStages, collaborationStages, expectedFiles, identity } from "./admission.ts";
import { abiFiles, buildEnv, compilerStage, elfDependencies, inputs, reference } from "./build.ts";
import {
  below,
  call,
  digest,
  env,
  jsonInteger,
  parseJson,
  read as readJson,
  root,
  sha,
  write,
  resolved,
} from "./io.ts";
import type { Artifact, Binary, Bundle, Inputs, Web } from "./types.ts";

export const receipts = [
  "before.json",
  "after.json",
  "build-env-inputs.json",
  "build-environment.json",
  "compile-receipt.json",
  "bundle.json",
  "web-receipt.json",
  "abi-receipt.json",
];
const collaborationConsumers = [
  "collaboration-flow",
  "collaboration-install-on",
  "collaboration-postgres-on",
  "collaboration-sqlite-on",
  "collaboration-postgres-off",
  "collaboration-sqlite-off",
];
const browserBinaries = ["fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"];
const collaborationBinaries = [...browserBinaries, "fvoci_server", "selected_install_lifetime"];
const packetModes = [0o555, 0o444, 0o600];

// Host facts a fixture cannot produce: the checkout, the full physical input
// collector (minutes of hashing), the pinned toolchain versions and free disk.
// Everything else (CI identity, Git state, compiler env, ABI files, ldd) runs
// for real against the checkout.
export interface HostFacts {
  checkout: string;
  inputs: () => Inputs;
  toolchain: () => { rustc: string; cargo: string; bun: string };
  freeBytes: (path: string) => number;
}
export const host: HostFacts = {
  checkout: root,
  inputs,
  toolchain: () => ({
    rustc: call(["rustc", "-Vv"]),
    cargo: call(["cargo", "-V"]),
    bun: call(["bun", "-v"]),
  }),
  freeBytes(path) {
    const disk = statfsSync(path);
    return disk.bavail * disk.bsize;
  },
};
function ldd(path: string): { exitCode: number; stdout: string; stderr: string } {
  const result = spawnSync(["ldd", path], { stdout: "pipe", stderr: "pipe" });
  return {
    exitCode: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}
const distOf = (h: HostFacts) => join(h.checkout, "apps/web/dist");

export const browser = () =>
  ["workspace-browser-build", "workspace-browser-shard"].includes(process.env.GITHUB_JOB ?? "");
const stages = (): readonly string[] => (browser() ? browserStages : collaborationStages);
const jobs = (): readonly [string, string] =>
  browser()
    ? ["workspace-browser-build", "workspace-browser-shard"]
    : ["collaboration-build", "collaboration-flow"];

// An absolute lexical path whose existing part has no symlink. Missing parents
// (a fresh consumer's target/debug or apps/web/dist) are allowed, as Python's
// Path.resolve() allowed them; install creates them.
function physicalPath(path: string): string {
  assert.ok(isAbsolute(path) && resolve(path) === path, "nonphysical handoff path");
  let existing = path;
  while (!existsSync(existing)) {
    assert.ok(!lstatOrNull(existing), "dangling handoff path");
    existing = dirname(existing);
  }
  assert.equal(realpathSync(existing), existing, "nonphysical handoff path");
  return path;
}
function lstatOrNull(path: string) {
  try {
    return lstatSync(path);
  } catch {
    return null;
  }
}
export function regular(path: string): string {
  physicalPath(path);
  assert.ok(lstatSync(path).isFile(), "nonregular handoff file");
  return path;
}
const read = (output: string, name: string) => readJson(regular(join(output, name)));

export type HandoffInputs = Inputs & {
  frontend_env?: Record<string, string>;
  frontend_env_files?: Record<string, string>;
};
export function buildInputs(h: HostFacts = host): HandoffInputs {
  // Same complete physical source/native/dependency/toolchain closure as the
  // collaboration packet, plus build-time frontend environment (hashes only).
  const value: HandoffInputs = h.inputs();
  if (!browser()) return value;
  const frontend = Object.entries(process.env)
    .filter(
      ([key]) =>
        /^(VITE_|BUN_|NODE_)/.test(key) ||
        ["CI", "API_PROXY_TARGET", "SOURCE_DATE_EPOCH", "TZ", "LANG", "LC_ALL"].includes(key),
    )
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const envFiles: Record<string, string> = {};
  for (const directory of [h.checkout, join(h.checkout, "apps/web")])
    for (const name of readdirSync(directory).sort())
      if (name.startsWith(".env")) {
        const path = join(directory, name);
        envFiles[relative(h.checkout, path)] = sha(regular(path));
      }
  return {
    ...value,
    frontend_env: Object.fromEntries(frontend.map(([key, item]) => [key, digest(item ?? "")])),
    frontend_env_files: envFiles,
  };
}
export function distFiles(h: HostFacts = host): Record<string, string> {
  const dist = distOf(h);
  assert.ok(
    existsSync(dist) && lstatSync(dist).isDirectory() && realpathSync(dist) === dist,
    "missing/nonphysical fresh dist",
  );
  const result: Record<string, string> = {};
  const visit = (directory: string) => {
    for (const name of readdirSync(directory)) {
      const path = join(directory, name),
        facts = lstatSync(path);
      assert.ok(!facts.isSymbolicLink(), "symlink dist asset");
      if (facts.isDirectory()) visit(path);
      else result[relative(dist, path)] = sha(regular(path));
    }
  };
  visit(dist);
  assert.ok(Object.keys(result).length, "empty fresh dist");
  return result;
}

export interface Context {
  platform?: { system: string; machine: string; os_release_sha256: string };
  repository: string;
  run: string;
  attempt: string;
  source: string;
  tree: string;
}
export function context(job: string, h: HostFacts = host): Context {
  assert.ok(process.env.CI === "true" && process.env.GITHUB_ACTIONS === "true");
  const actual = process.env.GITHUB_JOB ?? "";
  if (job === "collaboration-flow" && collaborationConsumers.includes(actual)) job = actual;
  assert.equal(actual, job, "wrong handoff job");
  if (browser()) {
    assert.equal(
      call(["git", "rev-parse", "HEAD"], h.checkout),
      env("GITHUB_SHA"),
      "checkout differs from tested SHA",
    );
    assert.equal(
      spawnSync(["git", "-c", "safe.directory=" + h.checkout, "diff", "--quiet", "HEAD"], {
        cwd: h.checkout,
      }).exitCode,
      0,
      "checkout has tracked changes",
    );
    assert.ok(/^[0-9]+$/.test(env("GITHUB_RUN_ID")) && /^[0-9]+$/.test(env("GITHUB_RUN_ATTEMPT")));
  } else identity("handoff", undefined, h.checkout);
  assert.equal(
    process.env.FVOCI_WEB_BUILD_PHASE,
    job === jobs()[0] ? "prepare" : "consume",
    "wrong web build phase",
  );
  const result: Context = {
    repository: env("GITHUB_REPOSITORY"),
    run: env("GITHUB_RUN_ID"),
    attempt: env("GITHUB_RUN_ATTEMPT"),
    source: call(["git", "rev-parse", "HEAD"], h.checkout),
    tree: call(["git", "rev-parse", "HEAD^{tree}"], h.checkout),
  };
  if (browser()) {
    assert.ok(
      process.platform === "linux" && process.arch === "x64",
      "unsupported browser platform",
    );
    result.platform = {
      system: "Linux",
      machine: "x86_64",
      os_release_sha256: digest(readFileSync("/etc/os-release")),
    };
  }
  return result;
}
export function paths(): { output: string; packet: string } {
  const output = env("FVOCI_SELECTED_CI_OUTPUT");
  assert.ok(isAbsolute(output) && realpathSync(output) === output);
  const facts = statSync(output);
  assert.ok(facts.isDirectory() && facts.uid === process.getuid?.());
  assert.equal(facts.mode & 0o777, 0o700);
  return { output, packet: physicalPath(env("FVOCI_WEB_BUILD_HANDOFF")) };
}

// Bounded hash-only evidence; never serialize paths, status or env values.
// Python json.dumps(sort_keys=True, separators=(",", ":")) bytes: keys in code
// point order and non-ASCII as lowercase \\uXXXX UTF-16 escapes.
const codePoints = (a: string, b: string) => {
  const x = Array.from(a, (c) => c.codePointAt(0) ?? 0),
    y = Array.from(b, (c) => c.codePointAt(0) ?? 0);
  for (let i = 0; i < Math.min(x.length, y.length); i++)
    if (x[i] !== y[i]) return (x[i] ?? 0) - (y[i] ?? 0);
  return x.length - y.length;
};
const ascii = (text: string) =>
  text.replace(/[\u0080-\uffff]/g, (c) => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0"));
function canonical(value: unknown): string {
  if (Array.isArray(value)) return "[" + value.map(canonical).join(",") + "]";
  if (value && typeof value === "object")
    return (
      "{" +
      Object.keys(value)
        .sort(codePoints)
        .map(
          (key) =>
            ascii(JSON.stringify(key)) + ":" + canonical((value as Record<string, unknown>)[key]),
        )
        .join(",") +
      "}"
    );
  return ascii(JSON.stringify(value ?? null));
}
const fingerprint = (value: unknown) => digest(canonical(value));
const fieldOf = (value: HandoffInputs, field: string): unknown =>
  (value as unknown as Record<string, unknown>)[field];
export function inputDiagnostics(
  output: string,
  before: HandoffInputs,
  after: HandoffInputs,
  current: HandoffInputs,
): void {
  const fields = [
    "head",
    "tree",
    "status",
    "tracked",
    "external",
    "untracked",
    ...(browser() ? ["frontend_env", "frontend_env_files"] : []),
  ];
  const limit = 512;
  const isMap = (item: unknown): item is Record<string, unknown> =>
    !!item && typeof item === "object" && !Array.isArray(item);
  const byPair = (a: string[], b: string[]) =>
    a.join() < b.join() ? -1 : a.join() > b.join() ? 1 : 0;
  const snapshot = (value: HandoffInputs) => {
    const record: Record<string, unknown> = {};
    for (const field of fields) {
      const item = fieldOf(value, field);
      const entry: Record<string, unknown> = { sha256: fingerprint(item) };
      if (isMap(item)) {
        const entries = Object.entries(item)
          .map(([key, child]) => [fingerprint(key), fingerprint(child)])
          .sort(byPair);
        entry.count = entries.length;
        entry.entries = Object.fromEntries(entries.slice(0, limit));
        entry.truncated = entries.length > limit;
      }
      record[field] = entry;
    }
    return { sha256: fingerprint(value), fields: record };
  };
  const delta = (left: HandoffInputs, right: HandoffInputs) => {
    const result: Record<string, unknown> = {};
    for (const field of fields) {
      const a = fieldOf(left, field),
        b = fieldOf(right, field);
      if (deepEquals(a, b)) continue;
      const record: Record<string, unknown> = {
        before_sha256: fingerprint(a),
        after_sha256: fingerprint(b),
      };
      if (isMap(a) && isMap(b)) {
        const changes = [...new Set([...Object.keys(a), ...Object.keys(b)])]
          .filter((key) => !(key in a) || !(key in b) || !deepEquals(a[key], b[key]))
          .map((key) => [fingerprint(key), fingerprint(a[key]), fingerprint(b[key])])
          .sort(byPair);
        record.count = changes.length;
        record.entries = changes.slice(0, limit);
        record.truncated = changes.length > limit;
      }
      result[field] = record;
    }
    return result;
  };
  for (const [name, value] of [
    ["before", before],
    ["after", after],
    ["current", current],
  ] as const)
    write(join(output, "handoff-input-" + name + "-safe.json"), snapshot(value));
  write(join(output, "handoff-input-delta-safe.json"), {
    schema: 1,
    entry_limit: limit,
    before_after: delta(before, after),
    after_current: delta(after, current),
  });
  process.stderr.write("handoff input mismatch: see bounded handoff-input-*-safe.json hashes\n");
}

// The actual emitted ordinary debug executable metadata.
export function qualifyBrowserBinary(record: Pick<Artifact, "target" | "profile">): void {
  assert.ok(deepEquals(record.target.kind, ["bin"]), "browser target must be bin");
  const profile = record.profile;
  assert.equal(profile.opt_level, "0", "browser opt_level must be present and 0");
  assert.ok(
    "debuginfo" in profile &&
      (profile.debuginfo === null ||
        (jsonInteger(profile, "debuginfo") && profile.debuginfo === 0)),
    "browser debuginfo must be present and 0/null",
  );
  assert.equal(profile.test, false, "browser test must be present and false");
}

interface Environment {
  rustc: string;
  cargo: string;
  bun: string;
  os_release: string;
  features: string[];
  nativeFeatures: string[];
  profile: string;
  devDebug: string | null;
  testDebug: string | null;
  target: string;
  sqlite: Record<string, string>;
}
const sqliteKeys = [
  "SQLITE3_LIB_DIR",
  "SQLITE3_INCLUDE_DIR",
  "SQLITE3_STATIC",
  "SQLITE3_NO_PKG_CONFIG",
];
export function qualify(output: string, h: HostFacts = host): Bundle {
  const before = read(output, "before.json") as HandoffInputs,
    after = read(output, "after.json") as HandoffInputs,
    current = buildInputs(h);
  if (!deepEquals(before, after) || !deepEquals(after, current))
    inputDiagnostics(output, before, after, current);
  assert.ok(
    deepEquals(before, after) && deepEquals(after, current),
    "current physical inputs differ",
  );
  assert.ok(
    deepEquals(read(output, "build-env-inputs.json"), buildEnv()),
    "compiler environment differs",
  );
  const receipt = read(output, "compile-receipt.json") as {
    source: string;
    tree: string;
    exit_code: number;
    full_inputs_unchanged: boolean;
    stages: Record<string, unknown>[];
  };
  assert.ok(receipt.source === before.head && receipt.tree === before.tree);
  assert.ok(receipt.exit_code === 0 && Object.is(receipt.full_inputs_unchanged, true));
  assert.equal(receipt.stages.length, stages().length);
  stages().forEach((name, index) => {
    const stage = receipt.stages[index];
    assert.ok(
      deepEquals(stage, {
        ...(read(output, name + "-stage.json") as object),
        compilerMessages: reference(join(output, name + "-compiler.jsonl")),
      }),
    );
    assert.equal(stage?.exit_code, 0);
  });
  const bundle = read(output, "bundle.json") as Bundle;
  assert.ok(bundle.source === before.head && bundle.tree === before.tree);
  assert.equal(bundle.full_inputs_unchanged, true);
  assert.equal(Object.keys(bundle.binaries).length, browser() ? 4 : 6);
  const names = new Set<string>();
  for (const [path, record] of Object.entries(bundle.binaries)) {
    const executable = regular(path);
    assert.ok(sha(executable) === record.sha256 && statSync(executable).size === record.bytes);
    const name = record.target.name;
    names.add(name);
    assert.equal(record.compiledSource, before.head);
    assert.equal(record.targetTriple, "x86_64-unknown-linux-gnu");
    assert.ok(
      deepEquals(
        [...record.features].sort(),
        name === "collab-engine"
          ? ["default", "worker"]
          : browser()
            ? name === "fvoci-e2e-fixture"
              ? ["db-tests"]
              : []
            : ["api-schema", "db-tests"],
      ),
    );
    if (browser()) qualifyBrowserBinary(record);
    else
      assert.equal(
        record.profile.test,
        ["fvoci_server", "selected_install_lifetime"].includes(name),
      );
  }
  assert.ok(
    deepEquals(names, new Set(browser() ? browserBinaries : collaborationBinaries)),
    "unexpected packet binaries",
  );
  const environment = read(output, "build-environment.json") as Environment;
  assert.ok(environment.rustc === h.toolchain().rustc && environment.cargo === h.toolchain().cargo);
  assert.ok(environment.bun === h.toolchain().bun && environment.bun === "1.4.2");
  assert.equal(environment.os_release, readFileSync("/etc/os-release", "utf8"));
  assert.ok(
    environment.rustc.includes("release: 1.98.1") &&
      environment.rustc.includes("host: x86_64-unknown-linux-gnu"),
  );
  assert.ok(
    deepEquals(environment.features, browser() ? [] : ["api-schema", "db-tests"]) &&
      deepEquals(environment.nativeFeatures, ["worker"]),
  );
  assert.ok(
    environment.profile === "debug" &&
      environment.devDebug === "0" &&
      environment.testDebug === "0",
  );
  assert.equal(environment.target, resolved(env("CARGO_TARGET_DIR")));
  assert.ok(
    deepEquals(environment.sqlite, Object.fromEntries(sqliteKeys.map((key) => [key, env(key)]))),
  );
  assert.ok(
    environment.sqlite.SQLITE3_STATIC === "1" && environment.sqlite.SQLITE3_NO_PKG_CONFIG === "1",
  );
  const web = read(output, "web-receipt.json") as Web;
  assert.ok(web.source === before.head && web.tree === before.tree);
  assert.ok(web.exit_code === 0 && Object.is(web.full_inputs_unchanged, true));
  assert.equal(web.servedDist, distOf(h));
  assert.ok(Object.keys(web.dist_files).length && deepEquals(web.dist_files, distFiles(h)));
  const abi = read(output, "abi-receipt.json") as {
    currentSource: string;
    currentELFDependenciesVerified: boolean;
    host_runtime_files: Record<string, string>;
    actualCurrentELFldd: Record<string, string>;
  };
  assert.ok(
    abi.currentSource === before.head && Object.is(abi.currentELFDependenciesVerified, true),
  );
  assert.ok(
    deepEquals(abi.host_runtime_files, Object.fromEntries(abiFiles().map((p) => [p, sha(p)]))),
  );
  assert.ok(
    deepEquals(
      new Set(Object.keys(abi.actualCurrentELFldd)),
      new Set(Object.keys(bundle.binaries)),
    ),
  );
  for (const executable of Object.keys(bundle.binaries)) {
    // Retain the producer's raw ldd receipt, but compare resolved dependency
    // identities: ASLR mapping addresses are not library inputs.
    const recorded = abi.actualCurrentELFldd[executable] ?? "";
    const actual = ldd(executable),
      out = actual.stdout,
      err = actual.stderr;
    assert.equal(actual.exitCode, 0, "current ELF ldd failed");
    assert.ok(!recorded.includes("not found") && !(out + err).includes("not found"));
    const recordedPaths = new Set(elfDependencies(0, recorded, "")),
      actualPaths = new Set(elfDependencies(0, out, err));
    assert.ok(
      recordedPaths.size && deepEquals(recordedPaths, actualPaths),
      "current ELF dependency set differs",
    );
    for (const path of actualPaths) {
      assert.ok(path in abi.host_runtime_files, "unqualified current ELF dependency");
      assert.equal(sha(path), abi.host_runtime_files[path], "current ELF library bytes differ");
    }
  }
  return bundle;
}

export function allowedDestination(path: string, output: string, h: HostFacts = host): boolean {
  physicalPath(path);
  const named = new Set([
    ...receipts,
    ...stages().flatMap((name) => [name + "-stage.json", name + "-compiler.jsonl"]),
  ]);
  return (
    (dirname(path) === output && named.has(basename(path))) ||
    [
      join(h.checkout, "target/debug"),
      join(h.checkout, "crates/collab-engine/target/debug"),
      ...(browser() ? [distOf(h)] : []),
    ].some((base) => below(path, base))
  );
}
function packetFiles(bundle: Bundle, output: string, h: HostFacts, web?: Web) {
  const files = expectedFiles(
    bundle,
    output,
    browser() ? (web ?? (read(output, "web-receipt.json") as Web)) : undefined,
    h.checkout,
  );
  const executables = new Set(Object.keys(bundle.binaries));
  const core = new Set(files.filter((p) => /\.(rlib|rmeta)$/.test(p)));
  return { files, executables, core };
}

interface Entry {
  path: string;
  sha256: string;
  bytes: number;
  mode: number;
  producer_inode: number;
  producer_mode: number;
}
interface Manifest extends Context {
  schema: number;
  producer_job: string;
  consumer_job: string;
  root: string;
  output: string;
  payload_sha256: string;
  entries: Record<string, Entry>;
}
// ustar headers come from node-tar; members are the fixed regular files only.
function packMember(fd: number, name: string, data: Uint8Array, mode: number): void {
  const header = Buffer.alloc(512);
  new Header({
    path: name,
    mode,
    size: data.length,
    type: "File",
    mtime: new Date(0),
    uid: 0,
    gid: 0,
  }).encode(header, 0);
  writeSync(fd, header);
  writeSync(fd, data);
  const padding = (512 - (data.length % 512)) % 512;
  if (padding) writeSync(fd, Buffer.alloc(padding));
}
export function exportPacket(h: HostFacts = host): void {
  const current = context(jobs()[0], h);
  const { output, packet } = paths();
  const bundle = qualify(output, h);
  assert.ok(!existsSync(packet));
  mkdirSync(packet, { mode: 0o700 });
  const { files, executables, core } = packetFiles(bundle, output, h);
  const entries: Record<string, Entry> = {};
  const archive = join(packet, "payload.tar");
  const fd = openSync(archive, "wx", 0o600);
  try {
    files.forEach((path, index) => {
      regular(path);
      assert.ok(allowedDestination(path, output, h), "foreign packet source");
      const data = readFileSync(path),
        facts = statSync(path),
        member = "f" + String(index).padStart(3, "0"),
        mode = executables.has(path) ? 0o555 : core.has(path) ? 0o444 : 0o600;
      entries[member] = {
        path,
        sha256: digest(data),
        bytes: data.length,
        mode,
        producer_inode: facts.ino,
        producer_mode: facts.mode & 0o7777,
      };
      packMember(fd, member, data, mode);
    });
    writeSync(fd, Buffer.alloc(1024));
  } finally {
    closeSync(fd);
  }
  const manifest: Manifest = {
    schema: 1,
    ...current,
    producer_job: jobs()[0],
    consumer_job: jobs()[1],
    root: h.checkout,
    output,
    payload_sha256: sha(archive),
    entries,
  };
  write(join(packet, "handoff.json"), manifest);
  appendFileSync(
    env("GITHUB_OUTPUT"),
    "handoff_sha256=" + sha(join(packet, "handoff.json")) + "\n",
  );
}

export function admit(h: HostFacts = host) {
  const current = context(jobs()[1], h);
  const { output, packet } = paths();
  const expected = env("FVOCI_WEB_BUILD_HANDOFF_SHA256");
  assert.match(expected, /^[0-9a-f]{64}$/);
  const manifestPath = regular(join(packet, "handoff.json"));
  assert.equal(sha(manifestPath), expected, "producer manifest digest differs");
  const manifest = parseJson(readFileSync(manifestPath, "utf8")) as Manifest;
  assert.ok(
    Object.entries(current).every(([key, value]) =>
      deepEquals((manifest as unknown as Record<string, unknown>)[key], value),
    ),
    "foreign/stale producer",
  );
  assert.ok(
    manifest.schema === 1 &&
      manifest.producer_job === jobs()[0] &&
      manifest.consumer_job === jobs()[1],
  );
  assert.ok(manifest.root === h.checkout && manifest.output === output, "no path relocation");
  assert.ok(h.freeBytes(output) >= 20_000_000_000, "consumer START disk floor");
  const archive = regular(join(packet, "payload.tar"));
  assert.equal(sha(archive), manifest.payload_sha256);
  return { current, output, expected, manifest, archive };
}

export interface Member {
  name: string;
  type: string;
  linkpath: string;
  size: number;
  mode: number;
  sha256: string;
  data?: Buffer;
}
// Streams every archive member through node-tar; keeps the bytes only of the
// members `keep` names, and hands each member's bytes to `sink` if given.
export async function members(
  archive: string,
  keep: (name: string) => boolean = () => false,
  sink?: (member: Omit<Member, "sha256" | "data">, chunk: Buffer) => void,
): Promise<Member[]> {
  // Plain POSIX/GNU tar only, as Python tarfile.open(mode="r:") required: the
  // first header carries the ustar magic. node-tar would gunzip transparently.
  const head = Buffer.alloc(512),
    fd = openSync(archive, "r");
  try {
    assert.equal(readSync(fd, head, 0, 512, 0), 512, "refused tar member: short archive");
  } finally {
    closeSync(fd);
  }
  assert.equal(
    head.subarray(257, 262).toString("latin1"),
    "ustar",
    "refused tar member: not plain tar",
  );
  const result: Member[] = [];
  const parser = new Parser({
    strict: true,
    onReadEntry: (entry: ReadEntry) => {
      const facts = {
        name: entry.path,
        type: entry.type,
        linkpath: entry.linkpath ?? "",
        size: entry.size,
        // The raw header field: ReadEntry.mode drops type bits (0100600 -> 0600).
        mode: entry.header.mode ?? -1,
      };
      const hash = new Bun.CryptoHasher("sha256"),
        chunks: Buffer[] = [];
      entry.on("data", (chunk: Buffer) => {
        hash.update(chunk);
        if (keep(facts.name)) chunks.push(chunk);
        sink?.(facts, chunk);
      });
      entry.on("end", () => {
        result.push({
          ...facts,
          sha256: hash.digest("hex"),
          ...(keep(facts.name) ? { data: Buffer.concat(chunks) } : {}),
        });
      });
    },
  });
  await new Promise<void>((done, fail) => {
    parser.on("end", () => {
      done();
    });
    parser.on("error", fail);
    parser.on("warn", (code: string) => {
      fail(new Error("refused tar member: " + code));
    });
    // Unsupported member types (e.g. SolarisACL "A") never reach onReadEntry.
    parser.on("ignoredEntry", () => {
      fail(new Error("refused tar member: unsupported type"));
    });
    createReadStream(archive).on("error", fail).pipe(parser);
  });
  return result;
}

export async function consume(h: HostFacts = host): Promise<void> {
  const { current, output, expected, manifest, archive } = admit(h);
  const entries = manifest.entries;
  assert.ok(Object.keys(entries).length);
  const destinations = Object.values(entries).map((e) => e.path);
  assert.equal(new Set(destinations).size, destinations.length);
  for (const name of receipts) assert.ok(destinations.includes(join(output, name)));
  const memberOf = (path: string) =>
    Object.keys(entries).find((name) => entries[name]?.path === path) ?? "";
  const bundleMember = memberOf(join(output, "bundle.json")),
    webMember = memberOf(join(output, "web-receipt.json"));
  const listed = await members(archive, (name) => name === bundleMember || name === webMember);
  assert.ok(
    listed.length === Object.keys(entries).length &&
      deepEquals(new Set(listed.map((m) => m.name)), new Set(Object.keys(entries))),
  );
  const decoded = (name: string) =>
    parseJson(listed.find((m) => m.name === name)?.data?.toString("utf8") ?? "");
  const packetBundle = decoded(bundleMember) as Bundle;
  const packetWeb = browser() ? (decoded(webMember) as Web) : undefined;
  assert.ok(
    deepEquals(
      new Set(destinations),
      new Set(packetFiles(packetBundle, output, h, packetWeb).files),
    ),
    "extra/missing packet destination",
  );
  for (const member of listed) {
    const entry = entries[member.name] as Entry;
    assert.ok(member.type === "File" && !member.linkpath && member.size === entry.bytes);
    assert.ok(packetModes.includes(entry.mode) && member.mode === entry.mode);
    assert.ok(
      allowedDestination(entry.path, output, h) && !existsSync(entry.path),
      "foreign/existing destination",
    );
    assert.equal(member.sha256, entry.sha256);
  }
  // Second pass writes each verified member exclusively, then re-hashes it.
  const open = new Map<string, number>();
  try {
    await members(archive, undefined, (member, chunk) => {
      const entry = entries[member.name] as Entry;
      let fd = open.get(member.name);
      if (fd === undefined) {
        mkdirSync(dirname(entry.path), { recursive: true });
        fd = openSync(entry.path, "wx", 0o600);
        open.set(member.name, fd);
      }
      writeSync(fd, chunk);
    });
    for (const [name, fd] of open) {
      const entry = entries[name] as Entry;
      fchmodSync(fd, entry.mode);
    }
  } finally {
    for (const fd of open.values()) closeSync(fd);
  }
  for (const entry of Object.values(entries)) {
    if (!existsSync(entry.path)) {
      // An empty member emits no data chunk.
      assert.equal(entry.bytes, 0);
      mkdirSync(dirname(entry.path), { recursive: true });
      const fd = openSync(entry.path, "wx", 0o600);
      fchmodSync(fd, entry.mode);
      closeSync(fd);
    }
    assert.equal(sha(entry.path), entry.sha256, "written packet member differs");
  }
  const bundle = qualify(output, h);
  for (const path of Object.keys(bundle.binaries))
    assert.equal(statSync(path).mode & 0o7777, 0o555);
  write(join(output, "handoff-consumed.json"), {
    ...current,
    producer_manifest_sha256: expected,
    full_current_physical_inputs_equal: true,
    fresh_dist_equal: true,
    received: Object.fromEntries(
      destinations.map((path) => {
        const facts = statSync(path);
        return [path, { sha256: sha(path), inode: facts.ino, mode: facts.mode & 0o7777 }];
      }),
    ),
  });
}

function osRelease(): Record<string, string> {
  return Object.fromEntries(
    readFileSync("/etc/os-release", "utf8")
      .split("\n")
      .filter((line) => line.includes("="))
      .map((line) => {
        const at = line.indexOf("=");
        return [line.slice(0, at), line.slice(at + 1).replace(/^"|"$/g, "")];
      }),
  );
}
export function browserBefore(h: HostFacts = host): void {
  context("workspace-browser-build", h);
  const { output } = paths();
  assert.ok(browser() && !existsSync(distOf(h)), "producer must start without dist");
  const release = osRelease();
  assert.ok(release.ID === "ubuntu" && release.VERSION_ID === "26.04");
  assert.equal(
    process.env.CARGO_BUILD_TARGET ?? "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
  );
  assert.equal(process.env.FVOCI_E2E_PROFILE ?? "debug", "debug", "browser packet is debug only");
  write(join(output, "build-env-inputs.json"), buildEnv());
  write(join(output, "before.json"), buildInputs(h));
  write(join(output, "build-environment.json"), {
    rustc: h.toolchain().rustc,
    cargo: h.toolchain().cargo,
    bun: h.toolchain().bun,
    os_release: readFileSync("/etc/os-release", "utf8"),
    target: resolved(env("CARGO_TARGET_DIR")),
    features: [],
    nativeFeatures: ["worker"],
    profile: "debug",
    devDebug: process.env.CARGO_PROFILE_DEV_DEBUG ?? null,
    testDebug: process.env.CARGO_PROFILE_TEST_DEBUG ?? null,
    sqlite: Object.fromEntries(sqliteKeys.map((key) => [key, env(key)])),
  });
}
export async function browserStage(name: string | undefined, h: HostFacts = host): Promise<number> {
  context("workspace-browser-build", h);
  const { output } = paths();
  assert.ok(name && (browserStages as readonly string[]).includes(name));
  const commands: Record<string, string[]> = {
    fixture: [
      "cargo",
      "build",
      "--locked",
      "--offline",
      "--bin",
      "fvoci-e2e-fixture",
      "--features",
      "db-tests",
    ],
    default: [
      "cargo",
      "build",
      "--locked",
      "--offline",
      "--bin",
      "fvoci-server",
      "--bin",
      "fvoci-migrate",
    ],
    engine: [
      "cargo",
      "build",
      "--locked",
      "--offline",
      "--manifest-path",
      join(h.checkout, "crates/collab-engine/Cargo.toml"),
      "--features",
      "worker",
      "--bin",
      "collab-engine",
    ],
  };
  const command = [...(commands[name] ?? []), "--message-format=json-render-diagnostics"];
  return await compilerStage(output, name, command, read(output, "before.json") as Inputs);
}
export function browserAfter(h: HostFacts = host): void {
  context("workspace-browser-build", h);
  const { output } = paths();
  const before = read(output, "before.json") as Inputs,
    after = buildInputs(h);
  write(join(output, "after.json"), after);
  assert.ok(deepEquals(before, after), "physical inputs changed during build");
  assert.ok(
    deepEquals(read(output, "build-env-inputs.json"), buildEnv()),
    "compiler environment changed",
  );
  const records: Record<string, unknown>[] = [],
    artifacts: Artifact[] = [];
  for (const name of browserStages) {
    const record = read(output, name + "-stage.json") as { exit_code: number };
    assert.equal(record.exit_code, 0);
    const log = join(output, name + "-compiler.jsonl");
    records.push({ ...record, compilerMessages: reference(log) });
    for (const line of readFileSync(log, "utf8").split("\n").filter(Boolean)) {
      const value = parseJson(line) as Artifact;
      if (value.reason === "compiler-artifact") artifacts.push(value);
    }
  }
  const bins: Record<string, Omit<Binary, "mode" | "inode" | "cargo_fresh">> = {};
  for (const name of browserBinaries) {
    const matches = artifacts.filter((a) => a.target.name === name && a.executable);
    assert.equal(matches.length, 1, "missing/ambiguous emitted browser binary");
    const artifact = matches[0] as Artifact;
    qualifyBrowserBinary(artifact);
    const path = regular(artifact.executable as string);
    bins[path] = {
      sha256: sha(path),
      bytes: statSync(path).size,
      compiledSource: before.head,
      targetTriple: "x86_64-unknown-linux-gnu",
      target: artifact.target,
      features: artifact.features,
      profile: artifact.profile,
    };
  }
  write(join(output, "bundle.json"), {
    source: before.head,
    tree: before.tree,
    full_inputs_unchanged: true,
    binaries: bins,
    compiler_artifacts: artifacts,
  });
  write(join(output, "compile-receipt.json"), {
    source: before.head,
    tree: before.tree,
    exit_code: 0,
    full_inputs_unchanged: true,
    stages: records,
  });
  write(join(output, "web-receipt.json"), {
    source: before.head,
    tree: before.tree,
    exit_code: 0,
    full_inputs_unchanged: true,
    dist_files: distFiles(h),
    servedDist: distOf(h),
    scope:
      "fresh producer dist; consumers require full identical physical inputs and exact asset hashes, without rebuilding",
  });
  const libraries = Object.fromEntries(abiFiles().map((p) => [p, sha(p)])),
    recorded: Record<string, string> = {};
  for (const path of Object.keys(bins)) {
    const result = ldd(path);
    const dependencies = elfDependencies(result.exitCode, result.stdout, result.stderr);
    assert.ok(dependencies.length && dependencies.every((p) => p in libraries));
    recorded[path] = result.stdout;
  }
  write(join(output, "abi-receipt.json"), {
    currentSource: before.head,
    currentELFDependenciesVerified: true,
    host_runtime_files: libraries,
    actualCurrentELFldd: recorded,
  });
}

export async function main(argv: string[]): Promise<number> {
  const [mode, stageName, ...extra] = argv;
  assert.ok(
    mode === "browser-stage" ? extra.length === 0 : stageName === undefined,
    "handoff usage",
  );
  switch (mode) {
    case "export":
      exportPacket();
      return 0;
    case "admit":
      admit();
      return 0;
    case "consume":
      await consume();
      return 0;
    case "browser-before":
      browserBefore();
      return 0;
    case "browser-stage":
      return await browserStage(stageName);
    case "browser-after":
      browserAfter();
      return 0;
    default:
      throw new Error("handoff usage");
  }
}
if (import.meta.main) {
  try {
    process.exitCode = await main(process.argv.slice(2));
  } catch (error) {
    // Fixed handoff refusals name only the failed check; no packet bytes.
    process.stderr.write(
      "web build handoff refused: " +
        (error instanceof Error ? error.message.slice(0, 512) : "thrown") +
        "\n",
    );
    process.exitCode = 1;
  }
}
