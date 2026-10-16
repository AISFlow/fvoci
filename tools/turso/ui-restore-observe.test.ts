import { afterEach, beforeEach, expect, spyOn, test } from "bun:test";
import { Database, type Changes } from "bun:sqlite";
import {
  chmodSync,
  existsSync,
  linkSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { observeRestoreCounts } from "./ui-restore-observe.ts";

let root: string;
let path: string;
let environment: Record<string, string>;
beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "fvoci-restore-counts-"));
  chmodSync(root, 0o700);
  path = join(root, "app.sqlite");
  const database = new Database(path);
  try {
    for (const table of ["projects", "documents", "tasks", "attachments", "revisions", "audit_log"])
      database.run(`CREATE TABLE ${table}(id INTEGER PRIMARY KEY)`);
    database.run("CREATE TABLE import_jobs(id INTEGER PRIMARY KEY,source TEXT)");
    database.run(
      "INSERT INTO documents VALUES(1); INSERT INTO import_jobs VALUES(1,'native-archive')",
    );
  } finally {
    database.close();
  }
  environment = {
    FVOCI_E2E_SELECTED_BACKEND: "sqlite",
    FVOCI_E2E_RESTORE_SQLITE_PATH: path,
    FVOCI_E2E_RESTORE_SQLITE_ROOT: root,
  };
});
afterEach(() => {
  rmSync(root, { recursive: true, force: true });
});

test("owned destination counts read through a real readonly transaction, rollback and close", () => {
  const bytes = readFileSync(path);
  const exec = Object.getOwnPropertyDescriptor(Database.prototype, "run")?.value as (
    this: Database,
    sql: string,
  ) => Changes;
  let readonlyDenied = false;
  const observed = spyOn(Database.prototype, "run").mockImplementation(function (
    this: Database,
    sql,
  ) {
    const result = exec.call(this, sql);
    if (sql === "BEGIN") {
      try {
        exec.call(this, "CREATE TABLE observer_must_not_write(id INTEGER)");
      } catch {
        readonlyDenied = true;
      }
    }
    return result;
  });
  const close = spyOn(Database.prototype, "close");
  try {
    const witness = observeRestoreCounts(environment);
    expect(witness.counts).toEqual({
      projects: 0,
      documents: 1,
      tasks: 0,
      attachments: 0,
      revisions: 0,
      jobs: 1,
      nativeJobs: 1,
      audit: 0,
    });
    expect(witness.sqlite.version).toMatch(/^\d+\.\d+\.\d+$/);
    expect(witness.sqlite.sourceId.length).toBeGreaterThan(40);
    expect(witness.rollback).toBe("confirmed");
    expect(witness.closed).toBe(true);
    expect(readonlyDenied).toBe(true);
    expect(observed.mock.calls.map(([sql]) => sql)).toEqual(["BEGIN", "ROLLBACK"]);
    expect(close).toHaveBeenCalledTimes(1);
    expect(readFileSync(path)).toEqual(bytes);
  } finally {
    close.mockRestore();
    observed.mockRestore();
  }
});

test("missing destination is refused without creating a database", () => {
  const missing = join(root, "missing.sqlite");
  expect(() =>
    observeRestoreCounts({ ...environment, FVOCI_E2E_RESTORE_SQLITE_PATH: missing }),
  ).toThrow();
  expect(existsSync(missing)).toBe(false);
});

test("missing schema refuses success and still rolls back and closes", () => {
  const database = new Database(path);
  database.run("DROP TABLE revisions");
  database.close();
  const exec = spyOn(Database.prototype, "run");
  const close = spyOn(Database.prototype, "close");
  try {
    expect(() => observeRestoreCounts(environment)).toThrow("RESTORE_OBSERVER_READ_FAILED");
    expect(exec.mock.calls.map(([sql]) => sql)).toEqual(["BEGIN", "ROLLBACK"]);
    expect(close).toHaveBeenCalledTimes(1);
  } finally {
    close.mockRestore();
    exec.mockRestore();
  }
});

test.each(["backend", "outside", "symlink", "hardlink", "directory-mode"])(
  "count witness refuses %s identity",
  (fault) => {
    if (fault === "backend") environment.FVOCI_E2E_SELECTED_BACKEND = "libsql-remote";
    if (fault === "outside") environment.FVOCI_E2E_RESTORE_SQLITE_ROOT = tmpdir();
    if (fault === "symlink") {
      const link = join(root, "link.sqlite");
      symlinkSync(path, link);
      environment.FVOCI_E2E_RESTORE_SQLITE_PATH = link;
    }
    if (fault === "hardlink") linkSync(path, join(root, "hard.sqlite"));
    if (fault === "directory-mode") chmodSync(root, 0o755);
    expect(() => observeRestoreCounts(environment)).toThrow();
  },
);

test("rollback failure cannot return a successful witness and close is still attempted", () => {
  const exec = Object.getOwnPropertyDescriptor(Database.prototype, "run")?.value as (
    this: Database,
    sql: string,
  ) => Changes;
  const observed = spyOn(Database.prototype, "run").mockImplementation(function (
    this: Database,
    sql,
  ) {
    if (sql === "ROLLBACK") throw new Error("injected rollback failure");
    return exec.call(this, sql);
  });
  const close = spyOn(Database.prototype, "close");
  try {
    expect(() => observeRestoreCounts(environment)).toThrow("RESTORE_OBSERVER_READ_FAILED");
    expect(close).toHaveBeenCalledTimes(1);
  } finally {
    close.mockRestore();
    observed.mockRestore();
  }
});
