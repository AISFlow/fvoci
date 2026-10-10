import { expect, test } from "bun:test";
import { YAML } from "bun";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
type Step = { name?: string; run?: string; uses?: string; with?: Record<string, unknown> };
const workflow = YAML.parse(readFileSync(join(root, ".github/workflows/rust.yml"), "utf8")) as {
  jobs: Record<string, { steps: Step[] } | undefined>;
};
const steps = (job: string): Step[] => {
  const found = workflow.jobs[job];
  if (!found) throw new Error(`rust.yml has no job ${job}`);
  return found.steps;
};
const step = (job: string, name: string): Step => {
  const found = steps(job).filter((value) => value.name === name);
  expect(found).toHaveLength(1);
  const [only] = found;
  if (!only) throw new Error(`rust.yml job ${job} has no step ${name}`);
  return only;
};
function flipByte(bytes: Uint8Array, index: number) {
  const value = bytes.at(index);
  if (value === undefined) throw new Error("fixture byte out of range");
  bytes[index] = value ^ 1;
}
const compress = step("postgres-build", "Compress sealed executable archives with zstd");
const hash = step("postgres-build", "Hash compressed executable archives");

function fixture() {
  const directory = mkdtempSync(join(root, ".c2b-fixture-"));
  const relative = `crates/collab-engine/target/${basename(directory)}-helper`;
  const destination = join(root, relative);
  const targetExisted = existsSync(dirname(destination));
  const env = {
    ...process.env,
    RUNNER_TEMP: directory,
    RUNNER_ARCH: "X64",
    GITHUB_SHA: command(["git", "rev-parse", "HEAD"]).stdout.toString().trim(),
    PYTHONDONTWRITEBYTECODE: "1",
  };
  const packet = join(directory, "rust-binaries");
  const stage = join(directory, "stage");
  const bytes = Buffer.from("sealed helper fixture bytes\n");
  mkdirSync(packet);
  mkdirSync(dirname(join(stage, relative)), { recursive: true });
  const manifest = {
    version: 1,
    context: {
      sha: env.GITHUB_SHA,
      workspace: root,
      arch: env.RUNNER_ARCH,
      os: readFileSync("/etc/os-release", "utf8"),
      rustc: command(["rustc", "-vV"]).stdout.toString(),
      sqlite: "a".repeat(64),
      profile: "dev-test-nodebug",
      build_environment: Object.fromEntries(
        [
          "CARGO_INCREMENTAL",
          "CARGO_PROFILE_DEV_DEBUG",
          "CARGO_PROFILE_TEST_DEBUG",
          "RUSTFLAGS",
          "CARGO_ENCODED_RUSTFLAGS",
          "CARGO_BUILD_TARGET",
        ].map((name) => [name, process.env[name] ?? null]),
      ),
    },
    entries: {
      "collab-engine": {
        path: relative,
        sha256: createHash("sha256").update(bytes).digest("hex"),
        record: {
          features: ["default", "worker"],
          profile: { opt_level: "0", debuginfo: 0, test: false },
          target: { name: "collab-engine", kind: ["bin"] },
          executable: destination,
        },
      },
    },
  };
  const run = (value: Step) => {
    if (value.run === undefined) throw new Error(`step ${String(value.name)} has no run script`);
    return Bun.spawnSync(
      [
        "bash",
        "-e",
        "-o",
        "pipefail",
        "-c",
        value.run.replaceAll("${{ steps.sqlite.outputs.cache_identity }}", "a".repeat(64)),
      ],
      { cwd: root, env, stdout: "pipe", stderr: "pipe" },
    );
  };
  const pack = (corrupt = false) => {
    const payload = Buffer.from(bytes);
    if (corrupt) flipByte(payload, 0);
    writeFileSync(join(stage, relative), payload);
    writeFileSync(join(stage, "manifest.json"), JSON.stringify(manifest));
    for (const cohort of ["postgres", "helper"]) {
      command([
        "tar",
        "-cf",
        join(packet, `${cohort}.tar`),
        "-C",
        stage,
        "manifest.json",
        relative,
      ]);
    }
    const result = run(compress);
    expect(result.exitCode).toBe(0);
    expect(result.stdout.toString()).toMatch(
      /stage=rust-binaries-compress finished at=\S+ elapsed_seconds=\d+ exit=0/,
    );
    for (const cohort of ["postgres", "helper"]) rmSync(join(packet, `${cohort}.tar`));
  };
  return {
    packet,
    bytes,
    destination,
    pack,
    run,
    cleanup() {
      rmSync(destination, { force: true });
      rmSync(directory, { recursive: true, force: true });
      if (
        !targetExisted &&
        existsSync(dirname(destination)) &&
        readdirSync(dirname(destination)).length === 0
      ) {
        rmdirSync(dirname(destination));
      }
    },
  };
}

function command(argv: string[]) {
  const result = Bun.spawnSync(argv, { cwd: root, stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) {
    throw new Error(
      `${argv.join(" ")} exit ${String(result.exitCode)}: ${result.stderr.toString()}`,
    );
  }
  return result;
}

function withFixture(body: (value: ReturnType<typeof fixture>) => void) {
  const value = fixture();
  try {
    body(value);
  } finally {
    value.cleanup();
  }
}

test("zstd transport retains the actual consumer's SHA256 admission", () => {
  withFixture((f) => {
    f.pack();
    const hashed = f.run(hash);
    expect(hashed.exitCode).toBe(0);
    expect(hashed.stdout.toString()).toMatch(
      /stage=rust-binaries-hash finished at=\S+ elapsed_seconds=\d+ exit=0/,
    );
    for (const cohort of ["postgres", "helper"]) {
      const expected = createHash("sha256")
        .update(readFileSync(join(f.packet, `${cohort}.tar.zst`)))
        .digest("hex");
      expect(hashed.stdout.toString()).toContain(`${expected}  ${cohort}.tar.zst`);
      const result = f.run(
        step(
          cohort === "postgres" ? "postgres" : "collaboration",
          `Decompress sealed ${cohort} executables`,
        ),
      );
      expect(result.exitCode).toBe(0);
    }
    const admitted = f.run(
      step(
        "collaboration",
        "Validate and restore finished helper executables (no rebuild fallback)",
      ),
    );
    expect(admitted.exitCode).toBe(0);
    expect(readFileSync(f.destination)).toEqual(f.bytes);
  });
});

test("a one-byte payload change in a valid zstd frame fails the existing digest check before writes", () => {
  withFixture((f) => {
    f.pack(true);
    expect(f.run(step("collaboration", "Decompress sealed helper executables")).exitCode).toBe(0);
    const refused = f.run(
      step(
        "collaboration",
        "Validate and restore finished helper executables (no rebuild fallback)",
      ),
    );
    expect(refused.exitCode).toBe(1);
    expect(refused.stderr.toString()).toContain("Rust binary digest/feature/profile/path mismatch");
    expect(existsSync(f.destination)).toBe(false);
  });
});

test("a one-byte transport change stops both consumers before admission", () => {
  withFixture((f) => {
    f.pack();
    for (const cohort of ["postgres", "helper"]) {
      const archive = join(f.packet, `${cohort}.tar.zst`);
      const bytes = readFileSync(archive);
      flipByte(bytes, bytes.length - 1);
      writeFileSync(archive, bytes);
      const refused = f.run(
        step(
          cohort === "postgres" ? "postgres" : "collaboration",
          `Decompress sealed ${cohort} executables`,
        ),
      );
      expect(refused.exitCode).not.toBe(0);
      expect(existsSync(f.destination)).toBe(false);
    }
  });
});

test("decompression refuses to overwrite an occupied raw archive", () => {
  withFixture((f) => {
    f.pack();
    const archive = join(f.packet, "helper.tar");
    writeFileSync(archive, "occupied");
    expect(f.run(step("collaboration", "Decompress sealed helper executables")).exitCode).not.toBe(
      0,
    );
    expect(readFileSync(archive, "utf8")).toBe("occupied");
  });
});

test("compression and hashing report separate failure timings without masking exit", () => {
  withFixture((f) => {
    for (const [value, name] of [
      [compress, "compress"],
      [hash, "hash"],
    ] as const) {
      const result = f.run(value);
      expect(result.exitCode).not.toBe(0);
      expect(result.stdout.toString()).toMatch(
        new RegExp(`stage=rust-binaries-${name} started at=\\S+`),
      );
      expect(result.stdout.toString()).toMatch(
        new RegExp(
          `stage=rust-binaries-${name} finished at=\\S+ elapsed_seconds=\\d+ exit=${String(result.exitCode)}`,
        ),
      );
    }
  });
});

test("both compressed uploads and consumers use the same format without skipping admission", () => {
  expect(compress.run).toContain("zstd -T0");
  expect(step("postgres-build", "Verify zstd CLI for executable handoff").run).toBe(
    "zstd --version",
  );
  for (const cohort of ["postgres", "helper"]) {
    const upload = step("postgres-build", `Upload ${cohort} finished executables`);
    expect(upload.with?.path).toBe(`\${{ runner.temp }}/rust-binaries/${cohort}.tar.zst`);
    expect(upload.with?.["compression-level"]).toBe(0);
    expect(upload.with?.["if-no-files-found"]).toBe("error");
    const job = cohort === "postgres" ? "postgres" : "collaboration";
    const download = step(
      job,
      `Download required ${cohort} executables for this SHA and architecture`,
    );
    const decompress = step(job, `Decompress sealed ${cohort} executables`);
    const validate = step(
      job,
      `Validate and restore finished ${cohort} executables (no rebuild fallback)`,
    );
    expect(steps(job).indexOf(download)).toBeLessThan(steps(job).indexOf(decompress));
    expect(steps(job).indexOf(decompress)).toBeLessThan(steps(job).indexOf(validate));
    expect(decompress.run).toContain("zstd --version");
    expect(decompress).not.toHaveProperty("continue-on-error");
    expect(validate).not.toHaveProperty("if");
  }
});
