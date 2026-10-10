import { deepEquals, spawnSync } from "bun";
import { strict as assert } from "node:assert";
import {
  closeSync,
  existsSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  statfsSync,
  statSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import { collaborationStages, identity, localAllocation } from "./admission.ts";
import {
  below,
  call,
  digest,
  env,
  files,
  inventory,
  observedExit,
  read,
  root,
  sha,
  spawnSelectedCommand,
  tool,
  write,
} from "./io.ts";
import type { Artifact, Binary, Bundle, Inputs, Stage } from "./types.ts";

export function elfDependencies(code: number, stdout: string, stderr: string): string[] {
  const text = stdout + "\n" + stderr;
  if (code === 0)
    return [...text.matchAll(/(?:=>\s*)?(\/[^\s]+)\s+\(/g)].map((match) =>
      realpathSync(match[1] as string),
    );
  if (
    code === 1 &&
    (text.includes("not a dynamic executable") || text.includes("statically linked"))
  )
    return [];
  throw new Error("ldd prerequisite failed");
}
export const abiFiles = () =>
  [
    "/lib64/ld-linux-x86-64.so.2",
    "/lib/x86_64-linux-gnu/libc.so.6",
    "/lib/x86_64-linux-gnu/libm.so.6",
    "/lib/x86_64-linux-gnu/libgcc_s.so.1",
  ].map((p) => realpathSync(p));
export const reference = (path: string) => ({ path: realpathSync(path), sha256: sha(path) });
export function buildEnv(): Record<string, string> {
  const names = Object.keys(process.env)
    .filter((name) =>
      /^(CARGO_|RUST|CC(?:_|$)|CXX(?:_|$)|HOST_CC$|TARGET_CC$|CFLAGS|CPPFLAGS|CXXFLAGS|LDFLAGS|LIBCLANG|BINDGEN_|SQLITE3_|PKG_CONFIG|CPATH$|C_INCLUDE_PATH$|CPLUS_INCLUDE_PATH$|LIBRARY_PATH$|PATH$)/.test(
        name,
      ),
    )
    .sort();
  assert.ok(!names.some((name) => /TOKEN|PASSWORD|SECRET|CREDENTIAL/.test(name)));
  assert.ok(!process.env.RUSTC_WRAPPER && !process.env.RUSTC_WORKSPACE_WRAPPER);
  return Object.fromEntries(names.map((name) => [name, digest(env(name))]));
}
export function inputs(): Inputs {
  const tracked = Object.fromEntries(
    call(["git", "ls-files", "-z"])
      .split("\0")
      .filter(Boolean)
      .map((p) => [p, sha(join(root, p))]),
  );
  const untracked = Object.fromEntries(
    call(["git", "ls-files", "--others", "--exclude-standard", "-z"])
      .split("\0")
      .filter((p) => p && statSync(join(root, p)).isFile())
      .map((p) => [p, sha(join(root, p))]),
  );
  const external: Record<string, string> = {};
  function add(path: string, excluded: string[] = []): void {
    const entries = statSync(path).isDirectory() ? files(path) : [path];
    for (const file of entries)
      if (!excluded.some((prefix) => below(file, prefix))) external[file] = sha(file);
  }
  const cargo = realpathSync(process.env.CARGO_HOME ?? join(homedir(), ".cargo"));
  const index = join(cargo, "registry/index");
  const caches = existsSync(index)
    ? readdirSync(index)
        .map((name) => join(index, name, ".cache"))
        .filter((p) => existsSync(p) && statSync(p).isDirectory())
    : [];
  for (const sub of ["registry", "git"])
    if (existsSync(join(cargo, sub))) add(join(cargo, sub), sub === "registry" ? caches : []);
  for (const config of [
    join(cargo, "config"),
    join(cargo, "config.toml"),
    join(process.env.FVOCI_SELECTED_CI_OUTPUT ?? "/nonexistent", "build-env-inputs.json"),
  ])
    if (existsSync(config)) add(config);
  assert.ok(
    !existsSync(join(cargo, "credentials")) && !existsSync(join(cargo, "credentials.toml")),
  );
  for (const directory of [
    join(root, "node_modules"),
    call(["rustc", "--print", "sysroot"]),
    env("LIBCLANG_PATH"),
    "/usr/include",
    dirname(env("SQLITE3_LIB_DIR")),
  ])
    add(directory);
  add(dirname(call(["cc", "-print-file-name=include"])));
  for (const name of ["cargo", "rustc", "cc", "ar", "ld", "bun"]) {
    const path = realpathSync(tool(name));
    add(path);
    const result = spawnSync(["ldd", path], { stdout: "pipe", stderr: "pipe" });
    for (const library of elfDependencies(
      observedExit(result),
      result.stdout.toString(),
      result.stderr.toString(),
    ))
      add(library);
  }
  add("/etc/os-release");
  for (const path of abiFiles()) add(path);
  // Unlike call(), status retains its LF: the current Python drivers compare it.
  const status = spawnSync(["git", "-c", "safe.directory=" + root, "status", "--short"], {
    cwd: root,
    stdout: "pipe",
    stderr: "pipe",
  });
  assert.equal(observedExit(status), 0);
  return {
    head: call(["git", "rev-parse", "HEAD"]),
    tree: call(["git", "rev-parse", "HEAD^{tree}"]),
    status: status.stdout.toString(),
    tracked,
    external,
    untracked,
  };
}
export function recordBefore(output: string): void {
  identity("record-before", output);
  assert.ok(!existsSync(join(output, "before.json")));
  const release = readFileSync("/etc/os-release", "utf8");
  assert.ok(/^ID="?ubuntu"?$/m.test(release) && /^VERSION_ID="?26\.04"?$/m.test(release));
  const rust = call(["rustc", "-Vv"]);
  assert.ok(rust.includes("release: 1.98.1") && rust.includes("host: x86_64-unknown-linux-gnu"));
  assert.equal(
    process.env.CARGO_BUILD_TARGET ?? "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
  );
  assert.equal(call(["bun", "-v"]), "1.4.2");
  assert.ok(process.env.SQLITE3_STATIC === "1" && process.env.SQLITE3_NO_PKG_CONFIG === "1");
  const disk = statfsSync(output);
  assert.ok(disk.bavail * disk.bsize >= 20_000_000_000);
  write(join(output, "build-env-inputs.json"), buildEnv());
  write(join(output, "before.json"), inputs());
  write(join(output, "build-environment.json"), {
    sqlite: Object.fromEntries(
      ["SQLITE3_LIB_DIR", "SQLITE3_INCLUDE_DIR", "SQLITE3_STATIC", "SQLITE3_NO_PKG_CONFIG"].map(
        (name) => [name, env(name)],
      ),
    ),
    rustc: rust,
    cargo: call(["cargo", "-V"]),
    bun: call(["bun", "-v"]),
    os_release: release,
    target: realpathSync(env("CARGO_TARGET_DIR")),
    features: ["api-schema", "db-tests"],
    nativeFeatures: ["worker"],
    profile: "debug",
    devDebug: process.env.CARGO_PROFILE_DEV_DEBUG ?? null,
    testDebug: process.env.CARGO_PROFILE_TEST_DEBUG ?? null,
    compilerBeforeRecorded: true,
  });
}
export function qualifyArtifacts(artifacts: Artifact[], before: Inputs): Record<string, Binary> {
  const bins: Record<string, Binary> = {};
  for (const name of [
    "fvoci-server",
    "fvoci-migrate",
    "fvoci-e2e-fixture",
    "fvoci_server",
    "selected_install_lifetime",
    "collab-engine",
  ]) {
    const matches = artifacts.filter(
      (a) =>
        a.target.name === name &&
        a.executable &&
        a.profile.test === ["fvoci_server", "selected_install_lifetime"].includes(name),
    );
    assert.ok(matches.length > 0 && new Set(matches.map((a) => a.executable)).size === 1);
    const last = matches.at(-1);
    assert.ok(last?.executable);
    assert.ok(
      matches.every(
        (a) =>
          deepEquals(a.target, last.target) &&
          deepEquals(a.features, last.features) &&
          deepEquals(a.profile, last.profile),
      ),
    );
    assert.ok(
      deepEquals(
        [...last.features].sort(),
        name === "collab-engine" ? ["default", "worker"] : ["api-schema", "db-tests"],
      ),
    );
    const facts = statSync(last.executable);
    assert.ok(Number.isSafeInteger(facts.ino));
    bins[last.executable] = {
      sha256: sha(last.executable),
      bytes: facts.size,
      mode: "0o" + facts.mode.toString(8),
      inode: facts.ino,
      compiledSource: before.head,
      targetTriple: "x86_64-unknown-linux-gnu",
      target: last.target,
      features: last.features,
      profile: last.profile,
      cargo_fresh: last.fresh,
    };
  }
  return bins;
}
export function recordAfter(output: string): void {
  identity("record-after", output);
  const before = read(join(output, "before.json")) as Inputs;
  assert.ok(deepEquals(read(join(output, "build-env-inputs.json")), buildEnv()));
  const after = inputs();
  write(join(output, "after.json"), after);
  assert.ok(deepEquals(before, after), "compile inputs changed");
  const stages: Stage[] = [],
    artifacts: Artifact[] = [];
  for (const name of collaborationStages) {
    const log = join(output, name + "-compiler.jsonl"),
      stage = read(join(output, name + "-stage.json")) as Stage;
    assert.ok(stage.exit_code === 0 && stage.source === before.head && stage.tree === before.tree);
    stages.push({ ...stage, compilerMessages: reference(log) });
    for (const line of readFileSync(log, "utf8").split("\n").filter(Boolean)) {
      const record: unknown = JSON.parse(line);
      if ((record as Artifact).reason === "compiler-artifact") artifacts.push(record as Artifact);
    }
  }
  const binaries = qualifyArtifacts(artifacts, before);
  write(join(output, "compile-receipt.json"), {
    source: before.head,
    tree: before.tree,
    exit_code: 0,
    full_inputs_unchanged: true,
    stages,
  });
  const bundle: Bundle = {
    source: before.head,
    tree: before.tree,
    full_inputs_unchanged: true,
    binaries,
    compiler_artifacts: artifacts,
  };
  write(join(output, "bundle.json"), bundle);
  const dist = join(root, "apps/web/dist"),
    assets = inventory(dist);
  assert.ok(Object.keys(assets).length);
  write(join(output, "web-receipt.json"), {
    source: before.head,
    tree: before.tree,
    exit_code: 0,
    full_inputs_unchanged: true,
    dist_files: assets,
    servedDist: dist,
    scope: "maintained current build before current source/native snapshot; actual emitted assets",
  });
  const libs = Object.fromEntries(abiFiles().map((path) => [path, sha(path)])),
    ldd: Record<string, string> = {};
  for (const path of Object.keys(binaries)) {
    const text = call(["ldd", path]);
    ldd[path] = text;
    const actual = elfDependencies(0, text, "");
    assert.ok(actual.length && actual.every((p) => p in libs));
  }
  const server = Object.keys(binaries).find((p) => p.endsWith("/fvoci-server"));
  assert.ok(server);
  write(join(output, "abi-receipt.json"), {
    currentSource: before.head,
    currentServerSha256: binaries[server]?.sha256,
    currentELFDependenciesVerified: true,
    actualCurrentELFldd: ldd,
    host_runtime_files: libs,
    os_release: readFileSync("/etc/os-release", "utf8"),
    scope:
      "current Ubuntu 26.04 build-host ABI evidence only; runtime uses its own image libraries",
  });
}
export async function stage(
  output: string,
  name: string | undefined,
  command: string[],
): Promise<number> {
  identity("stage", output);
  assert.ok(name && (collaborationStages as readonly string[]).includes(name) && command.length);
  if (process.env.FVOCI_SELECTED_EXECUTION_MODE === "orca-local")
    assert.ok(deepEquals(command, localAllocation("stage").stageCommands[name]));
  const before = read(join(output, "before.json")) as Inputs;
  assert.equal(call(["git", "rev-parse", "HEAD"]), before.head);
  return await compilerStage(output, name, command, before);
}
// One compiler command with exclusive JSON/stderr logs and its stage receipt.
// SIGINT kills and reaps the direct child and records exit 130.
export async function compilerStage(
  output: string,
  name: string,
  command: string[],
  before: Inputs,
): Promise<number> {
  const start = performance.now(),
    out = openSync(join(output, name + "-compiler.jsonl"), "wx"),
    err = openSync(join(output, name + "-stderr.log"), "wx");
  let code: number;
  const controller = new AbortController();
  const interrupt = () => {
    controller.abort(new Error("selected compiler stage interrupted"));
  };
  process.once("SIGINT", interrupt);
  try {
    const child = spawnSelectedCommand(command, process.env, out, err, controller.signal);
    const exitCode = await child.exited;
    code = controller.signal.aborted
      ? 130
      : observedExit({ exitCode, signalCode: child.signalCode ?? undefined });
  } catch (error) {
    if (!controller.signal.aborted) throw error;
    code = 130;
  } finally {
    process.removeListener("SIGINT", interrupt);
    closeSync(out);
    closeSync(err);
  }
  write(join(output, name + "-stage.json"), {
    source: before.head,
    tree: before.tree,
    command,
    exit_code: code,
    seconds: (performance.now() - start) / 1000,
  });
  return code;
}
