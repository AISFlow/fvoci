// Black-box checks of the database URLs scripts/perf/perf-inner.sh hands to
// fvoci-migrate. docker and fvoci-migrate are stubs; the migrate stub records
// the URLs and stops the run before any server starts.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const SCRIPT = resolve(import.meta.dir, "../../scripts/perf/perf-inner.sh");
let work = "";

function stub(path: string, body: string): void {
  writeFileSync(path, `#!/bin/sh\n${body}\n`);
  chmodSync(path, 0o755);
}

beforeEach(() => {
  work = mkdtempSync(join(tmpdir(), "perf-inner-"));
  for (const dir of ["bin", "release", "run", "out"]) mkdirSync(join(work, dir));
  stub(join(work, "bin/docker"), `echo "$*" >>'${work}/docker.calls'`);
  stub(
    join(work, "release/fvoci-migrate"),
    `printf '%s\\n%s\\n' "$DATABASE_URL" "$DATABASE_APP_URL" >'${work}/urls'; exit 1`,
  );
});

afterEach(() => {
  rmSync(work, { recursive: true, force: true });
});

function runInner(adminUrl: string) {
  const result = spawnSync("bash", [SCRIPT], {
    encoding: "utf8",
    env: {
      PATH: `${join(work, "bin")}:${process.env.PATH ?? ""}`,
      ROOT: work,
      RUN_DIR: join(work, "run"),
      RELEASE: join(work, "release"),
      FVOCI_PERF_OUT: join(work, "out"),
      FVOCI_PERF_DATASET: "fixture",
      FVOCI_TEST_PG_CONTAINER: "fvoci-stub-pg",
      TEST_DATABASE_URL: adminUrl,
    },
  });
  const urlsPath = join(work, "urls");
  const calls = join(work, "docker.calls");
  return {
    status: result.status,
    stderr: result.stderr,
    urls: existsSync(urlsPath) ? readFileSync(urlsPath, "utf8").trimEnd().split("\n") : [],
    dockerCalls: existsSync(calls) ? readFileSync(calls, "utf8").trimEnd().split("\n") : [],
  };
}

const DB = "fvoci_perf_[0-9a-f]{16}";
const APP = `fvoci_app_${DB}:[0-9a-f]{32}`;

describe("perf-inner database URLs", () => {
  test("admin URL keeps credentials and query; app URL keeps host and port", () => {
    const r = runInner("postgres://postgres:s3cret@127.0.0.1:41234/postgres?sslmode=disable#f");
    expect(r.status).not.toBe(0);
    const [admin = "", app = ""] = r.urls;
    expect(admin).toMatch(
      new RegExp(`^postgres://postgres:s3cret@127\\.0\\.0\\.1:41234/(${DB})\\?sslmode=disable#f$`),
    );
    const db = admin.match(new RegExp(DB))?.[0] ?? "missing";
    expect(app).toMatch(new RegExp(`^postgres://${APP}@127\\.0\\.0\\.1:41234/${db}$`));
    expect(app.startsWith(`postgres://fvoci_app_${db}:`)).toBe(true);
    expect(r.dockerCalls).toHaveLength(1);
    expect(r.dockerCalls[0]).toContain(`CREATE DATABASE "${db}"`);
  });

  test("defaults the host and port and lowercases the host", () => {
    expect(runInner("postgres://u:p@DB.Example:6543/postgres").urls[1]).toMatch(
      new RegExp(`^postgres://${APP}@db\\.example:6543/${DB}$`),
    );
    expect(runInner("postgres://u:p@localhost/postgres").urls[1]).toMatch(
      new RegExp(`^postgres://${APP}@localhost:5432/${DB}$`),
    );
    expect(runInner("postgres://u:p@:5432").urls[1]).toMatch(
      new RegExp(`^postgres://${APP}@127\\.0\\.0\\.1:5432/${DB}$`),
    );
  });

  test("an IPv6 admin host stays bracketed in the app URL", () => {
    expect(runInner("postgres://u:p@[::1]:5432/postgres").urls[1]).toMatch(
      new RegExp(`^postgres://${APP}@\\[::1\\]:5432/${DB}$`),
    );
  });

  for (const [label, url, message] of [
    ["no scheme", "127.0.0.1:5432/postgres", "TEST_DATABASE_URL is not a URL"],
    [
      "non-numeric port",
      "postgres://u:p@127.0.0.1:abc/postgres",
      "TEST_DATABASE_URL has an invalid port",
    ],
    ["empty", "", "TEST_DATABASE_URL"],
  ] as const) {
    test(`refuses a malformed admin URL (${label}) before creating anything`, () => {
      const r = runInner(url);
      expect(r.status).not.toBe(0);
      expect(r.stderr).toContain(message);
      expect(r.dockerCalls).toEqual([]);
      expect(r.urls).toEqual([]);
      expect(r.stderr).not.toContain("p@");
    });
  }
});
