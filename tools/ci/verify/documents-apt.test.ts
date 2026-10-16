import { afterAll, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { get, isMapping, parseWorkflow } from "./load.ts";
import { CACHE_PIN } from "./registry.ts";

const root = resolve(import.meta.dir, "../../..");
const workflow = parseWorkflow(
  "documents.yml",
  readFileSync(join(root, ".github/workflows/documents.yml"), "utf8"),
).data;
const steps = get(get(get(workflow, "jobs"), "native-extraction"), "steps");
const list = Array.isArray(steps) ? steps.filter(isMapping) : [];
const prepareAt = list.findIndex((step) => get(step, "id") === "sqlite");
const prepareRun = get(list[prepareAt], "run");
const run = typeof prepareRun === "string" ? prepareRun : "";
const scratch = mkdtempSync(join(tmpdir(), "documents-apt-"));
afterAll(() => {
  rmSync(scratch, { recursive: true, force: true });
});

function armInstaller(): string {
  const body = run.match(/ {2}arm64\)\n([\s\S]*?)\n {4};;/)?.[1];
  expect(body, "arm64 must have its own verified package installer").toBeDefined();
  return body ?? "exit 99";
}

function exercise(
  mode:
    | "miss"
    | "hit"
    | "corrupt-hit"
    | "corrupt-download"
    | "install-failure"
    | "unpack-failure"
    | "configure-failure",
) {
  const directory = mkdtempSync(join(scratch, "case-"));
  const cache = join(directory, "fvoci-clang18-arm64");
  const bin = join(directory, "bin");
  mkdirSync(bin);
  mkdirSync(cache);
  const fixture = "verified package fixture\n";
  const digest = createHash("sha256").update(fixture).digest("hex");
  const body = armInstaller();
  const packages = [...body.matchAll(/([a-f0-9]{64}) ([a-z0-9+_.-]+\.deb)/g)];
  expect(packages).toHaveLength(4);
  // Keep the production shell and checksum utility; substitute only package
  // payloads and their digests so this process-boundary test stays offline.
  const script = body.replace(/[a-f0-9]{64}(?= [a-z0-9+_.-]+\.deb)/g, digest);
  if (mode === "hit" || mode === "corrupt-hit") {
    for (const [, , name] of packages) {
      writeFileSync(join(cache, name ?? ""), mode === "hit" ? fixture : "corrupt\n");
    }
  }
  const stub = join(bin, "stub.ts");
  writeFileSync(
    stub,
    `#!${process.execPath}\n` +
      'import { appendFileSync, writeFileSync } from "node:fs";\n' +
      'import { basename } from "node:path";\n' +
      "const command = basename(process.argv[1]);\n" +
      'appendFileSync(process.env.CALLS, JSON.stringify([command, ...process.argv.slice(2)]) + "\\n");\n' +
      'if (command === "curl") {\n' +
      '  const at = process.argv.indexOf("--output");\n' +
      "  if (at < 0) process.exit(98);\n" +
      "  writeFileSync(process.argv[at + 1], process.env.PAYLOAD);\n" +
      '} else if (command === "sudo") {\n' +
      '  const stage = process.argv[2] === "dpkg" ? "unpack" : process.argv[3] === "update" ? "update" : "configure";\n' +
      "  process.exit(stage === process.env.FAIL_STAGE ? 42 : 0);\n" +
      "}\n",
    { mode: 0o755 },
  );
  for (const name of ["curl", "sudo"]) copyFileSync(stub, join(bin, name));
  const callsFile = join(directory, "calls.jsonl");
  writeFileSync(callsFile, "");
  const result = Bun.spawnSync(["bash", "-euo", "pipefail"], {
    stdin: new TextEncoder().encode(script),
    env: {
      PATH: `${bin}:/usr/bin:/bin`,
      RUNNER_TEMP: directory,
      CALLS: callsFile,
      PAYLOAD: mode === "corrupt-download" ? "corrupt\n" : fixture,
      FAIL_STAGE:
        mode === "install-failure"
          ? "update"
          : mode === "unpack-failure"
            ? "unpack"
            : mode === "configure-failure"
              ? "configure"
              : "",
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  const calls = readFileSync(callsFile, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line): string[] => {
      const value: unknown = JSON.parse(line);
      if (!Array.isArray(value) || !value.every((part: unknown) => typeof part === "string")) {
        throw new Error("invalid command call fixture");
      }
      return value;
    });
  return { code: result.exitCode, calls, stderr: result.stderr.toString() };
}

test("arm64 restores and saves only the exact versioned package cache", () => {
  const restoreAt = list.findIndex((step) => get(step, "id") === "clang18_arm64_cache");
  const restore = list[restoreAt];
  expect(restoreAt).toBeGreaterThan(-1);
  expect(prepareAt).toBeGreaterThan(-1);
  expect(restoreAt).toBeLessThan(prepareAt);
  expect(get(restore, "if")).toBe("runner.arch == 'ARM64'");
  expect(get(restore, "uses")).toBe(`actions/cache/restore@${CACHE_PIN}`);
  const config = get(restore, "with");
  expect(get(config, "path")).toBe("${{ runner.temp }}/fvoci-clang18-arm64");
  expect(get(config, "key")).toBe(
    "v1-clang18-debs-ubuntu-26.04-${{ runner.arch }}-1.99.0-18.1.8-20ubuntu8-arm64",
  );
  expect(get(config, "restore-keys")).toBeUndefined();
  const saveAt = list.findIndex(
    (step) => get(step, "name") === "Save verified arm64 libclang packages",
  );
  const save = list[saveAt];
  expect(saveAt).toBeGreaterThan(prepareAt);
  expect(get(save, "uses")).toBe(`actions/cache/save@${CACHE_PIN}`);
  expect(get(save, "if")).toBe(
    "runner.arch == 'ARM64' && github.ref == 'refs/heads/main' && steps.clang18_arm64_cache.outputs.cache-hit != 'true'",
  );
  expect(get(get(save, "with"), "path")).toBe(get(config, "path"));
  expect(get(get(save, "with"), "key")).toBe(get(config, "key"));
});

test("x64 retains signed primary apt installation and arm64 resolves missing Depends", () => {
  const x64 = run.match(/ {2}amd64\)\n([\s\S]*?)\n {4};;/)?.[1];
  expect(x64).toContain("Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg");
  expect(x64).toContain(
    "printf '%s\\n' 'https://archive.ubuntu.com/ubuntu/' | sudo tee /etc/apt/apt-mirrors.txt >/dev/null",
  );
  expect(x64).toContain("sudo apt-get update");
  expect(x64).toContain(
    "sudo apt-get install -y --no-install-recommends gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8",
  );
  expect(armInstaller()).toContain("sudo apt-get update");
  expect(run.split("esac")[1]).not.toContain("apt-get");
});

test("arm64 cache miss verifies four large packages before dependency resolution", () => {
  const result = exercise("miss");
  expect(result.code, result.stderr).toBe(0);
  expect(result.calls.filter(([command]) => command === "curl")).toHaveLength(4);
  for (const call of result.calls.filter(([command]) => command === "curl")) {
    expect(
      call.some(
        (arg) =>
          arg.startsWith(
            "https://ports.ubuntu.com/ubuntu-ports/pool/universe/l/llvm-toolchain-18/",
          ) && arg.endsWith("_18.1.8-20ubuntu8_arm64.deb"),
      ),
    ).toBe(true);
  }
  const installs = result.calls.filter(([command]) => command === "sudo");
  expect(installs).toHaveLength(3);
  expect(installs[0]?.slice(1)).toEqual(["apt-get", "update"]);
  expect(installs[1]?.slice(1, 4)).toEqual(["dpkg", "--unpack", "--"]);
  expect(installs[1]?.slice(4)).toHaveLength(4);
  expect(installs[2]?.slice(1, 9)).toEqual([
    "apt-get",
    "install",
    "-y",
    "--no-install-recommends",
    "--fix-broken",
    "--no-remove",
    "--no-upgrade",
    "--",
  ]);
  expect(installs[2]?.slice(9)).toEqual(installs[1]?.slice(4));
});

test("arm64 cache hit verifies large packages without curl downloads", () => {
  const result = exercise("hit");
  expect(result.code, result.stderr).toBe(0);
  expect(result.calls.map(([command]) => command)).toEqual(["sudo", "sudo", "sudo"]);
});

for (const mode of ["corrupt-hit", "corrupt-download"] as const) {
  test(`arm64 ${mode} fails closed before installation`, () => {
    const result = exercise(mode);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toContain("checksum did NOT match");
    expect(result.calls.some(([command]) => command === "sudo")).toBe(false);
    expect(result.calls.filter(([command]) => command === "curl")).toHaveLength(
      mode === "corrupt-hit" ? 0 : 1,
    );
  });
}

test("arm64 installer errors propagate", () => {
  expect(exercise("install-failure").code).toBe(42);
});

for (const mode of ["unpack-failure", "configure-failure"] as const) {
  test(`arm64 ${mode} propagates without later installation stages`, () => {
    const result = exercise(mode);
    expect(result.code).toBe(42);
    expect(result.calls.filter(([command]) => command === "sudo")).toHaveLength(
      mode === "unpack-failure" ? 2 : 3,
    );
  });
}

// Real Ubuntu resolute arm64 package metadata, reduced to dependency/identity
// fields from main + universe binary-arm64/Packages.xz in the signed Release.
// Their SHA-256 values are 537715817d9ed77cc919f5123edb91a48d9ae97abf05588a8f18208cc7227e08
// and 0a68c22478773d9681e098a3a1e650464aa707c4a557b4c3020678d92dfb53c0.
// This invokes the host apt resolver against an isolated file-only repository;
// No sudo stub, network or ARM executable is involved. Scriptless tiny debs
// exercise local acquisition/unpack in an isolated dpkg root, never the host.
test("arm64 installs verified local files and resolves only indexed Depends", () => {
  const directory = mkdtempSync(join(scratch, "resolver-"));
  const repository = join(directory, "repository");
  for (const path of [repository, "state/lists/partial", "cache/archives/partial"]) {
    mkdirSync(path === repository ? path : join(directory, path), { recursive: true });
  }
  const index = readFileSync(join(import.meta.dir, "documents-arm64.Packages"), "utf8");
  for (const [, digest, filename] of armInstaller().matchAll(
    /([a-f0-9]{64}) ([a-z0-9+_.-]+\.deb)/g,
  )) {
    const stanza = index
      .split("\n\n")
      .find((entry) =>
        entry
          .split("\n")
          .some((line) => line.startsWith("Filename: ") && line.endsWith(`/${filename ?? ""}`)),
      );
    expect(stanza).toBeDefined();
    expect(stanza).toContain(`SHA256: ${digest ?? ""}\n`);
  }
  writeFileSync(join(repository, "Packages"), index);
  writeFileSync(join(directory, "sources.list"), `deb [trusted=yes] file:${repository} ./\n`);
  const dpkgRoot = join(directory, "root");
  const status = join(dpkgRoot, "var/lib/dpkg/status");
  mkdirSync(join(dpkgRoot, "var/lib/dpkg"), { recursive: true });
  writeFileSync(status, "");
  const options = [
    `Dir::Etc::sourcelist=${join(directory, "sources.list")}`,
    "Dir::Etc::sourceparts=-",
    "Dir::Etc::main=-",
    "Dir::Etc::parts=-",
    `Dir::State=${join(directory, "state")}`,
    `Dir::State::status=${status}`,
    `Dir::Cache=${join(directory, "cache")}`,
    "APT::Architecture=arm64",
    "APT::Architectures::=arm64",
    "Acquire::Languages=none",
    "Debug::NoLocking=true",
  ].flatMap((option) => ["-o", option]);
  const apt = (...args: string[]) =>
    Bun.spawnSync(["apt-get", ...options, ...args], {
      env: { PATH: "/usr/bin:/bin", LC_ALL: "C" },
      stdout: "pipe",
      stderr: "pipe",
    });
  let fixtureIndex = index;
  const localDebs = [...armInstaller().matchAll(/[a-f0-9]{64} ([a-z0-9+_.-]+\.deb)/g)].map(
    (match) => {
      const filename = match[1];
      if (filename === undefined) throw new Error("missing pinned package filename");
      const entry = index.split("\n\n").find((stanza) => stanza.includes(`/${filename}\n`));
      if (!entry) throw new Error("missing indexed package");
      const name = entry.match(/^Package: (.+)$/m)?.[1];
      if (!name) throw new Error("missing package name");
      const contents = join(directory, `contents-${name}`);
      mkdirSync(join(contents, "DEBIAN"), { recursive: true });
      mkdirSync(join(contents, "usr/share/fvoci-ci-fixture"), { recursive: true });
      writeFileSync(
        join(contents, "DEBIAN/control"),
        entry
          .split("\n")
          .filter((line) => line !== "" && !/^(Filename|Size|SHA256):/.test(line))
          .join("\n") + "\nMaintainer: Fixture <fixture@invalid>\n",
      );
      writeFileSync(
        join(contents, `usr/share/fvoci-ci-fixture/${name}`),
        "verified-local-payload\n",
      );
      const file = join(directory, filename);
      const build = Bun.spawnSync(["dpkg-deb", "--build", "--root-owner-group", contents, file], {
        stdout: "pipe",
        stderr: "pipe",
      });
      expect(build.exitCode, build.stderr.toString()).toBe(0);
      const bytes = readFileSync(file);
      // Both origins describe identical package bytes, as the real signed index
      // and the verified cache do. Keep all dependency fields unchanged.
      fixtureIndex = fixtureIndex.replace(
        entry,
        entry
          .trimEnd()
          .replace(/^Size: .+$/m, `Size: ${String(bytes.length)}`)
          .replace(/^SHA256: .+$/m, `SHA256: ${createHash("sha256").update(bytes).digest("hex")}`) +
          "\nMaintainer: Fixture <fixture@invalid>",
      );
      return { name, file };
    },
  );
  expect(localDebs).toHaveLength(4);
  writeFileSync(join(repository, "Packages"), fixtureIndex);
  const update = apt("update");
  expect(update.exitCode, update.stderr.toString()).toBe(0);
  const body = armInstaller();
  const directUnpack = body.includes('sudo dpkg --unpack -- "${debs[@]}"');
  if (directUnpack) {
    const unpack = Bun.spawnSync(
      [
        "dpkg",
        `--root=${dpkgRoot}`,
        "--force-not-root",
        "--force-architecture",
        "--unpack",
        "--",
        ...localDebs.map(({ file }) => file),
      ],
      { stdout: "pipe", stderr: "pipe" },
    );
    expect(unpack.exitCode, unpack.stderr.toString()).toBe(0);
  }
  const install = body.match(/^ {4}sudo apt-get install ([^\n]+)/m)?.[1];
  expect(install).toBeDefined();
  const flags = (install ?? "").replace(/ -- "\$\{debs\[@\]\}"$/, "").split(" ");
  const result = apt("--print-uris", "install", ...flags, ...localDebs.map(({ file }) => file));
  expect(result.exitCode, result.stderr.toString()).toBe(0);
  const output = result.stdout.toString();
  for (const { name, file } of localDebs) {
    // A local operand alone is insufficient: apt 3.2 substitutes the index
    // Filename for an equal package/version. Direct unpack must consume the
    // verified file, and the remaining apt acquire plan must omit that package.
    if (!directUnpack) {
      const acquiredUri = output
        .split("\n")
        .find((line) => line.startsWith("'file:") && line.includes(`/${name}_`))
        ?.split("'")[1];
      expect(acquiredUri === undefined ? undefined : new URL(acquiredUri).href, name).toBe(
        pathToFileURL(file).href,
      );
    }
    expect(readFileSync(join(dpkgRoot, `usr/share/fvoci-ci-fixture/${name}`), "utf8")).toBe(
      "verified-local-payload\n",
    );
    expect(
      output.split("\n").some((line) => line.startsWith("'file:") && line.includes(`/${name}_`)),
      name,
    ).toBe(false);
  }
  for (const name of [
    "libstdc++-15-dev",
    "libgcc-15-dev",
    "libobjc-15-dev",
    "libobjc4",
    "libedit2",
    "libffi8",
    "libxml2-16",
    "libzstd1",
    "libtinfo6",
  ]) {
    // --no-download suppresses these fetches even though the resolver needs
    // them. Assert the actual fetch plan, not just a successful simulation.
    expect(
      output
        .split("\n")
        .some(
          (line) => line.startsWith("'file:") && decodeURIComponent(line).includes(`/${name}_`),
        ),
      name,
    ).toBe(true);
  }
});
