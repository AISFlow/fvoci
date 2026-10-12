import { afterAll, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
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
  expect(body, "arm64 must have its own verified offline installer").toBeDefined();
  return body ?? "exit 99";
}

function exercise(mode: "miss" | "hit" | "corrupt-hit" | "corrupt-download" | "install-failure") {
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
      '} else if (command === "sudo") process.exit(Number(process.env.INSTALL_EXIT));\n',
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
      INSTALL_EXIT: mode === "install-failure" ? "42" : "0",
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
    "v1-clang18-debs-ubuntu-26.04-${{ runner.arch }}-1.98.1-18.1.8-20ubuntu8-arm64",
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

test("x64 retains signed primary apt installation and arm64 has no apt update", () => {
  const x64 = run.match(/ {2}amd64\)\n([\s\S]*?)\n {4};;/)?.[1];
  expect(x64).toContain("Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg");
  expect(x64).toContain(
    "printf '%s\\n' 'https://archive.ubuntu.com/ubuntu/' | sudo tee /etc/apt/apt-mirrors.txt >/dev/null",
  );
  expect(x64).toContain("sudo apt-get update");
  expect(x64).toContain(
    "sudo apt-get install -y --no-install-recommends gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8",
  );
  expect(armInstaller()).not.toContain("apt-get update");
  expect(run.split("esac")[1]).not.toContain("apt-get");
});

test("arm64 cache miss downloads four packages and installs without network", () => {
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
  expect(installs).toHaveLength(1);
  expect(installs[0]?.slice(1, 7)).toEqual([
    "apt-get",
    "install",
    "-y",
    "--no-download",
    "--no-install-recommends",
    "--",
  ]);
  expect(installs[0]?.slice(7)).toHaveLength(4);
});

test("arm64 cache hit verifies packages with no downloads", () => {
  const result = exercise("hit");
  expect(result.code, result.stderr).toBe(0);
  expect(result.calls.map(([command]) => command)).toEqual(["sudo"]);
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
