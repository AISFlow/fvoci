// Black-box checks of scripts/perf/perf-inner.sh with stub docker, migrate,
// server, curl and bun: the database URLs it hands to fvoci-migrate and the
// run metadata JSON it writes.
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
  for (const dir of ["bin", "release", "run", "out", "apps/web"]) {
    mkdirSync(join(work, dir), { recursive: true });
  }
  stub(
    join(work, "bin/docker"),
    `echo "$*" >>'${work}/docker.calls'
case "$*" in *"SHOW server_version"*) printf '%s\\n' "$STUB_PG_VERSION" ;; esac`,
  );
  // Stops the run after recording the URLs unless the test asks for a full run.
  stub(
    join(work, "release/fvoci-migrate"),
    `[ -f '${work}/urls' ] || printf '%s\\n%s\\n' "$DATABASE_URL" "$DATABASE_APP_URL" >'${work}/urls'
[ -n "$STUB_FULL_RUN" ]`,
  );
  stub(
    join(work, "release/fvoci-server"),
    "echo 'fvoci-server listening on http://127.0.0.1:9'; exec sleep 60",
  );
  stub(join(work, "bin/curl"), "exit 0");
  stub(join(work, "bin/bun"), "exit 0");
});

afterEach(() => {
  rmSync(work, { recursive: true, force: true });
});

function runInner(adminUrl: string, extra: Record<string, string> = {}) {
  const result = spawnSync("bash", [SCRIPT], {
    encoding: "utf8",
    timeout: 60_000,
    env: {
      PATH: `${join(work, "bin")}:${process.env.PATH ?? ""}`,
      ROOT: work,
      RUN_DIR: join(work, "run"),
      RELEASE: join(work, "release"),
      FVOCI_PERF_OUT: join(work, "out"),
      FVOCI_PERF_DATASET: "fixture",
      FVOCI_TEST_PG_CONTAINER: "fvoci-stub-pg",
      TEST_DATABASE_URL: adminUrl,
      ...extra,
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
const appUrl = (hostPort: string) => new RegExp(`^postgres://${APP}@${hostPort}/${DB}$`);

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

  for (const [url, hostPort] of [
    ["postgres://u:p@DB.Example:6543/postgres", "db\\.example:6543"],
    ["postgres://u:p@localhost/postgres", "localhost:5432"],
    ["postgres://u:p@:5432", "127\\.0\\.0\\.1:5432"],
    ["postgres://u:p@127.0.0.1:05432/postgres", "127\\.0\\.0\\.1:5432"],
    ["postgres://u:p@127.0.0.1:65535/postgres", "127\\.0\\.0\\.1:65535"],
    ["postgres://u:p@127.0.0.1:1/postgres", "127\\.0\\.0\\.1:1"],
    ["postgres://u:p@[::1]:5432/postgres", "\\[::1\\]:5432"],
    ["postgres://u:p@[::1]/postgres", "\\[::1\\]:5432"],
    ["postgres://u:p@[::FFFF:192.0.2.1]:5432/postgres", "\\[::ffff:192\\.0\\.2\\.1\\]:5432"],
    ["postgres://u:p@[1:2:3:4:5:6:7:8]:5432/postgres", "\\[1:2:3:4:5:6:7:8\\]:5432"],
    ["postgres://u:p@[1:2:3:4:5:6:7::]:5432/postgres", "\\[1:2:3:4:5:6:7::\\]:5432"],
  ] as const) {
    test(`app URL host and port for ${url}`, () => {
      const r = runInner(url);
      expect(r.urls[1]).toMatch(appUrl(hostPort));
    });
  }

  for (const [label, url, message] of [
    ["no scheme", "127.0.0.1:5432/postgres", "is not a printable ASCII URL"],
    ["empty", "", "TEST_DATABASE_URL"],
    ["space", "postgres://u:p@127.0.0.1:5432/post gres", "is not a printable ASCII URL"],
    ["non-ASCII host", "postgres://u:p@hö:5432/postgres", "is not a printable ASCII URL"],
    ["non-numeric port", "postgres://u:p@127.0.0.1:abc/postgres", "has an invalid port"],
    ["port 0", "postgres://u:p@127.0.0.1:0/postgres", "has a port outside 1-65535"],
    ["port 65536", "postgres://u:p@127.0.0.1:65536/postgres", "has a port outside 1-65535"],
    ["huge port", "postgres://u:p@127.0.0.1:99999999999999999999/postgres", "outside 1-65535"],
    ["non-IP bracket", "postgres://u:p@[abc]:5432/postgres", "has an invalid IPv6 literal"],
    ["IPv4 in brackets", "postgres://u:p@[192.0.2.1]:5432/postgres", "invalid IPv6 literal"],
    ["two ::", "postgres://u:p@[1::2::3]:5432/postgres", "has an invalid IPv6 literal"],
    ["nine groups", "postgres://u:p@[1:2:3:4:5:6:7:8:9]/postgres", "invalid IPv6 literal"],
    ["eight groups and ::", "postgres://u:p@[1:2:3:4:5:6:7::8]/postgres", "invalid IPv6 literal"],
    ["long group", "postgres://u:p@[::12345]/postgres", "has an invalid IPv6 literal"],
    ["bad IPv4 tail", "postgres://u:p@[::ffff:1.2.3.256]/postgres", "invalid IPv6 literal"],
    ["zone ID", "postgres://u:p@[fe80::1%25eth0]:5432/postgres", "invalid IPv6 literal"],
    ["unclosed bracket", "postgres://u:p@[::1:5432/postgres", "has an unclosed IPv6 bracket"],
    ["text after bracket", "postgres://u:p@[::1]x:5432/postgres", "has text after its IPv6"],
    ["stray bracket", "postgres://u:p@h]:5432/postgres", "has a bracket in its host"],
    ["userinfo bracket", "postgres://u[:p@127.0.0.1:5432/postgres", "bracket in its userinfo"],
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

describe("perf-inner run metadata", () => {
  const ADMIN = "postgres://postgres:pw@127.0.0.1:5432/postgres";
  const runJson = () => join(work, "out/run-fixture.json");

  for (const version of [
    "18.3",
    "18.3 (Debian 18.3-1.pgdg13+1)",
    'we"ird\\ver',
    "ctl\u0001\u0008\u001b\u001fend",
    "del\u007f",
  ]) {
    test(`writes valid JSON for server_version ${JSON.stringify(version)}`, () => {
      const r = runInner(ADMIN, { STUB_FULL_RUN: "1", STUB_PG_VERSION: version });
      expect(r.status).toBe(0);
      const text = readFileSync(runJson(), "utf8");
      expect(text.endsWith("}")).toBe(true);
      const record = JSON.parse(text) as Record<string, unknown>;
      expect(Object.keys(record)).toEqual([
        "dataset_run_started",
        "postgres_server_version",
        "network",
        "server",
      ]);
      expect(record.postgres_server_version).toBe(version.replace(/[\t\n\v\f\r ]/g, ""));
      expect(record.dataset_run_started).toMatch(
        /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}\+00:00$/,
      );
    });
  }

  test("refuses a non-ASCII server_version instead of writing it", () => {
    const r = runInner(ADMIN, { STUB_FULL_RUN: "1", STUB_PG_VERSION: "18.3é" });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain("server_version is not ASCII");
    expect(existsSync(runJson())).toBe(false);
  });
});
