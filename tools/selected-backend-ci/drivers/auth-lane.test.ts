// The auth-session lane pieces the PostgreSQL and SQLite drivers share: the
// owned SMTP sink lifecycle and the read-only SQLite auth event observation.
import { afterAll, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { spawn } from "bun";
import { chmodSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { authEventRows } from "../../turso/ui-auth-session.ts";
import { listenerAbsent, sha } from "../io.ts";
import { portClosed } from "./common.ts";
import { authServerEnvironment, startAuthSink, stopAuthSink } from "./postgres.ts";
import { authEvents } from "./sqlite.ts";

const scratch = mkdtempSync(join(tmpdir(), "fvoci-auth-lane-"));
afterAll(() => {
  rmSync(scratch, { recursive: true, force: true });
});
const refusal = (promise: Promise<unknown>) =>
  promise.then(
    () => "admitted",
    (error: unknown) => (error instanceof Error ? error.message : "thrown"),
  );
function run(): string {
  const path = mkdtempSync(join(scratch, "run-"));
  chmodSync(path, 0o700);
  return path;
}

test("the owned sink listens on its reported port and retires on SIGTERM", async () => {
  const directory = run();
  const sink = await startAuthSink(directory, process.execPath);
  try {
    expect(sink.port).toBeGreaterThan(0);
    expect(await portClosed(sink.port)).toBe(false);
    expect(listenerAbsent(sink.port)).toBe(false);
    expect(sink.capture).toBe(join(directory, "smtp-capture.private.jsonl"));
    expect(authServerEnvironment(sink)).toEqual({
      FVOCI_COLLAB_MAX_ROOMS: "3",
      SMTP_HOST: "127.0.0.1",
      SMTP_PORT: String(sink.port),
      SMTP_FROM: "auth-session@example.invalid",
    });
  } finally {
    expect(await stopAuthSink(sink)).toEqual({ exit: -15, forced: false, listenerAbsent: true });
  }
});

test("the kernel listener check accepts only a real TCP port", () => {
  for (const port of [0, -1, 65536, 1.5]) expect(() => listenerAbsent(port)).toThrow();
});

test("a sink that exits before its port file is refused and its log is closed", async () => {
  const directory = run();
  const refused = startAuthSink(directory, process.execPath, (_args, log) =>
    spawn([process.execPath, "--no-env-file", "--eval", "process.exit(3)"], {
      stdin: "ignore",
      stdout: log,
      stderr: log,
    }),
  );
  expect(await refusal(refused)).toBe("owned SMTP sink exited before its port file");
  expect(existsSync(join(directory, "smtp-port.private"))).toBe(false);
});

test("a reported port that nothing listens on is refused", async () => {
  const directory = run();
  const refused = startAuthSink(
    directory,
    process.execPath,
    (_args, log) =>
      spawn(
        [
          process.execPath,
          "--no-env-file",
          "--eval",
          `require("node:fs").writeFileSync(${JSON.stringify(join(directory, "smtp-port.private"))}, "9"); setTimeout(() => {}, 60000);`,
        ],
        { stdin: "ignore", stdout: log, stderr: log },
      ),
    () => Promise.resolve(true),
  );
  expect(await refusal(refused)).toBe("owned SMTP sink is not listening");
});

test("the server names wait for the owned sink port", () => {
  expect(authServerEnvironment(null)).toEqual({
    FVOCI_COLLAB_MAX_ROOMS: "3",
    SMTP_HOST: "127.0.0.1",
    SMTP_PORT: "",
    SMTP_FROM: "auth-session@example.invalid",
  });
});

test("SQLite auth events are read without changing the database", () => {
  const path = join(run(), "app.sqlite");
  const database = new Database(path);
  database.run(`CREATE TABLE events (
    id BLOB PRIMARY KEY NOT NULL, seq INTEGER NOT NULL UNIQUE, workspace_id BLOB,
    actor_user_id BLOB, verb TEXT NOT NULL, target_type TEXT, target_id BLOB,
    payload TEXT NOT NULL DEFAULT '{}', channel TEXT NOT NULL DEFAULT 'web') STRICT`);
  const member = new Uint8Array(16).fill(7);
  const insert = database.prepare(
    "INSERT INTO events (id,seq,workspace_id,actor_user_id,verb,target_type,target_id,payload) VALUES (?,?,?,?,?,?,?,?)",
  );
  insert.run(new Uint8Array(16).fill(1), 3, null, member, "auth.logout", null, null, "{}");
  insert.run(new Uint8Array(16).fill(2), 1, null, member, "auth.login", null, null, "{}");
  insert.run(
    new Uint8Array(16).fill(3),
    2,
    new Uint8Array(16).fill(9),
    member,
    "doc.update",
    "document",
    member,
    "{}",
  );
  database.close();
  const before = sha(path);
  const rows = authEventRows(authEvents(path), "sqlite");
  expect(rows.map((row) => [row.seq, row.verb, row.actor])).toEqual([
    ["1", "auth.login", "07".repeat(16)],
    ["3", "auth.logout", "07".repeat(16)],
  ]);
  expect(sha(path)).toBe(before);
  expect(() => authEvents(join(run(), "missing.sqlite"))).toThrow();
});
