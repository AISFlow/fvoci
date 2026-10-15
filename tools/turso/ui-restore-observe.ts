// A supplemental count witness over the owned destination. Bun SQLite is
// recorded separately from the product's sealed SQLite/native inputs.
import { Database } from "bun:sqlite";
import { lstatSync, realpathSync, statSync } from "node:fs";
import { dirname, isAbsolute } from "node:path";
import { require, UiError } from "./ui-common.ts";
import { uid } from "../selected-backend-ci/io.ts";

const COUNTS = `SELECT
  (SELECT count(*) FROM projects) AS projects,
  (SELECT count(*) FROM documents) AS documents,
  (SELECT count(*) FROM tasks) AS tasks,
  (SELECT count(*) FROM attachments) AS attachments,
  (SELECT count(*) FROM revisions) AS revisions,
  (SELECT count(*) FROM import_jobs) AS jobs,
  (SELECT count(*) FROM import_jobs WHERE source='native-archive') AS nativeJobs,
  (SELECT count(*) FROM audit_log) AS audit`;
export interface RestoreCounts {
  projects: number;
  documents: number;
  tasks: number;
  attachments: number;
  revisions: number;
  jobs: number;
  nativeJobs: number;
  audit: number;
}
export interface CountWitness {
  counts: RestoreCounts;
  identity: { device: string; inode: string };
  sqlite: { version: string; sourceId: string };
  bun: string;
  readonly: true;
  rollback: "confirmed";
  closed: true;
}

export function observeRestoreCounts(
  environment: Record<string, string | undefined> = process.env,
): CountWitness {
  require(process.versions.bun === "1.4.3", "RESTORE_OBSERVER_BUN_PIN_REQUIRED");
  require(environment.FVOCI_E2E_SELECTED_BACKEND === "sqlite", "RESTORE_OBSERVER_BACKEND_REFUSED");
  const path = environment.FVOCI_E2E_RESTORE_SQLITE_PATH;
  const root = environment.FVOCI_E2E_RESTORE_SQLITE_ROOT;
  require(typeof path === "string" &&
    typeof root === "string" &&
    isAbsolute(path) &&
    isAbsolute(root), "RESTORE_OBSERVER_PATH_REQUIRED");
  require(realpathSync(root) === root &&
    realpathSync(path) === path &&
    dirname(path) === root, "RESTORE_OBSERVER_PATH_REFUSED");
  const directory = lstatSync(root),
    file = lstatSync(path);
  require(directory.isDirectory() &&
    directory.uid === uid() &&
    (directory.mode & 0o777) === 0o700 &&
    file.isFile() &&
    file.uid === uid() &&
    file.nlink === 1, "RESTORE_OBSERVER_IDENTITY_REFUSED");
  const identity = { device: String(file.dev), inode: String(file.ino) };
  const database = new Database(path, { readonly: true, create: false, strict: true });
  let original: unknown = null;
  let counts: RestoreCounts | null = null;
  let sqlite: { version: string; sourceId: string } | null = null;
  const cleanup: string[] = [];
  try {
    database.run("BEGIN");
    counts = database.query<RestoreCounts, []>(COUNTS).get();
    sqlite = database
      .query<{ version: string; sourceId: string }, []>(
        "SELECT sqlite_version() AS version,sqlite_source_id() AS sourceId",
      )
      .get();
    require(counts !== null &&
      Object.values(counts).every((value) => Number.isSafeInteger(value) && value >= 0) &&
      sqlite !== null, "RESTORE_OBSERVER_COUNT_REFUSED");
  } catch (error) {
    original = error;
  }
  try {
    if (database.inTransaction) database.run("ROLLBACK");
    require(!database.inTransaction, "RESTORE_OBSERVER_ROLLBACK_FAILED");
  } catch {
    cleanup.push("RESTORE_OBSERVER_ROLLBACK_FAILED");
  }
  try {
    database.close();
  } catch {
    cleanup.push("RESTORE_OBSERVER_CLOSE_FAILED");
  }
  if (original !== null || cleanup.length) throw new UiError("RESTORE_OBSERVER_READ_FAILED");
  const fresh = statSync(path);
  require(String(fresh.dev) === identity.device &&
    String(fresh.ino) === identity.inode, "RESTORE_OBSERVER_IDENTITY_CHANGED");
  require(counts !== null && sqlite !== null, "RESTORE_OBSERVER_COUNT_REFUSED");
  return {
    counts,
    identity,
    sqlite,
    bun: process.versions.bun,
    readonly: true,
    rollback: "confirmed",
    closed: true,
  };
}
