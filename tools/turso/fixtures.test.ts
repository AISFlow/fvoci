import { afterEach, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import { FIXTURE_SUITES, fixtureArgv, fixtureEnv, main } from "./fixtures.ts";

// Pinned itself: the child here is always a recording shell script, never bun.
const roots: string[] = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function checkout(skip?: string): string {
  const root = mkdtempSync(join(tmpdir(), "fvoci-turso-fixtures-"));
  roots.push(root);
  for (const suite of FIXTURE_SUITES) {
    if (suite === skip) continue;
    mkdirSync(join(root, dirname(suite)), { recursive: true });
    writeFileSync(join(root, suite), "");
  }
  return root;
}

function fakeBun(root: string, tail: string): string {
  const path = join(root, "fake-bun");
  const seen = join(root, "seen");
  writeFileSync(
    path,
    "#!/bin/sh\n" +
      `{ pwd; printf '%s\\n' "$@"; printf 'secret=%s\\n' "\${FAKE_SECRET-absent}"; } > '${seen}'\n` +
      tail +
      "\n",
  );
  chmodSync(path, 0o700);
  return path;
}

async function quiet(run: () => Promise<number>): Promise<[number, string]> {
  const errors: string[] = [];
  const err = process.stderr.write.bind(process.stderr);
  process.stderr.write = (chunk: string) => errors.push(chunk) > 0;
  try {
    return [await run(), errors.join("")];
  } finally {
    process.stderr.write = err;
  }
}

describe("admission fixtures", () => {
  test("the pinned suites are every Turso suite plus the workflow literal checks", () => {
    const turso = readdirSync(import.meta.dir)
      .filter((name) => name.endsWith(".test.ts"))
      .map((name) => "tools/turso/" + name)
      .sort();
    expect<string[]>([...FIXTURE_SUITES].sort()).toEqual(
      [...turso, "tools/ci/verify/registry.test.ts"].sort(),
    );
    const repository = join(import.meta.dir, "../..");
    for (const suite of FIXTURE_SUITES) expect(existsSync(join(repository, suite))).toBe(true);
  });

  test("only PATH and TMPDIR reach the suites", () => {
    const ambient = {
      PATH: "/usr/bin:/bin",
      HOME: "/home/fixture",
      FVOCI_TEST_TURSO_AUTH_TOKEN: "FAKE_PRIVATE_TOKEN",
      GITHUB_TOKEN: "FAKE_PRIVATE_TOKEN",
      FVOCI_SELECTED_EXECUTION_MODE: "orca-local",
    };
    expect(fixtureEnv(ambient)).toEqual({ PATH: "/usr/bin:/bin" });
    expect(fixtureEnv({ ...ambient, TMPDIR: "/tmp/t3" })).toEqual({
      PATH: "/usr/bin:/bin",
      TMPDIR: "/tmp/t3",
    });
  });

  test("bun test runs every pinned path from the root without .env or a foreign bunfig", () => {
    expect(fixtureArgv("/bin/bun", "/repo")).toEqual([
      "/bin/bun",
      "--no-env-file",
      "--config=/repo/bunfig.toml",
      "test",
      ...FIXTURE_SUITES.map((suite) => "./" + suite),
    ]);
  });

  test("a missing suite is refused before any child", async () => {
    for (const suite of [FIXTURE_SUITES[0], FIXTURE_SUITES[FIXTURE_SUITES.length - 1]]) {
      const root = checkout(suite);
      const bun = fakeBun(root, "exit 0");
      const [code, stderr] = await quiet(() => main({ PATH: "/usr/bin:/bin" }, root, bun));
      expect(code).toBe(1);
      expect(stderr).toBe("TURSO_FIXTURE_SUITE_MISSING " + suite + "\n");
      expect(existsSync(join(root, "seen"))).toBe(false);
    }
  });

  test("the child's exit status is the result; a signal is a failure", async () => {
    for (const [tail, expected] of [
      ["exit 0", 0],
      ["exit 7", 7],
      ["kill -TERM $$", 1],
    ] as const) {
      const root = checkout();
      const bun = fakeBun(root, tail);
      const env = { PATH: "/usr/bin:/bin", FAKE_SECRET: "FAKE_PRIVATE_TOKEN" };
      const [code] = await quiet(() => main(env, root, bun));
      expect(code).toBe(expected);
      expect(readFileSync(join(root, "seen"), "utf8")).toBe(
        [
          root,
          "--no-env-file",
          "--config=" + join(root, "bunfig.toml"),
          "test",
          ...FIXTURE_SUITES.map((suite) => "./" + suite),
          "secret=absent",
          "",
        ].join("\n"),
      );
    }
  });
});
