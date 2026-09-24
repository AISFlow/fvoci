import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { once } from "node:events";
import {
  ownedServerChildEnv,
  processGroupMembers,
  readProcMember,
  signalOwnedGroup,
  SERVER_BIN_MISSING,
  startOwnedServer,
} from "./collab-restart.ts";

test("owned server start requires the exact inner-script binary path", async () => {
  const previous = process.env.FVOCI_E2E_SERVER_BIN;
  delete process.env.FVOCI_E2E_SERVER_BIN;
  try {
    await assert.rejects(startOwnedServer(), { message: SERVER_BIN_MISSING });
  } finally {
    if (previous === undefined) delete process.env.FVOCI_E2E_SERVER_BIN;
    else process.env.FVOCI_E2E_SERVER_BIN = previous;
  }
});

test("cleanup refuses a stale group identity and reaps only its owned child", { timeout: 5_000 }, async () => {
  const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
    detached: true,
    stdio: "ignore",
  });
  const exited = once(child, "exit");
  await once(child, "spawn");
  const member = readProcMember(child.pid!);
  assert.ok(member);
  try {
    assert.throws(
      () => signalOwnedGroup(member.pgrp, [{ ...member, starttime: "stale" }]),
      /cannot prove ownership/,
    );
    assert.equal(readProcMember(member.pid)?.starttime, member.starttime);
    signalOwnedGroup(member.pgrp, [member]);
    await exited;
    assert.equal(readProcMember(member.pid), null);
  } finally {
    if (readProcMember(member.pid)?.starttime === member.starttime) {
      child.kill("SIGKILL");
      await exited;
    }
  }
});

test("child env keeps app/migration URLs and strips fixture-only admin aliases", () => {
  const env = ownedServerChildEnv("127.0.0.1:4321", {
    PATH: "/bin",
    DATABASE_URL: "postgres://owner/db",
    DATABASE_APP_URL: "postgres://app/db",
    FVOCI_E2E_ADMIN_DATABASE_URL: "postgres://admin-secret/db",
    TEST_DATABASE_URL: "postgres://test-secret/db",
    FVOCI_TEST_PG_CONTAINER: "fvoci-rust-test-pg",
    PASSWORD_PEPPER_KEYS: '{"test":"aa"}',
    PASSWORD_PEPPER_ACTIVE_KEY_ID: "test",
    FVOCI_STATIC_DIR: "/tmp/dist",
    FVOCI_COLLAB_ENGINE: "/tmp/collab-engine",
    FVOCI_E2E_SERVER_BIN: "/tmp/fvoci-server",
    PEPPER: "should-not-copy",
  });
  assert.equal(env.DATABASE_URL, "postgres://owner/db");
  assert.equal(env.DATABASE_APP_URL, "postgres://app/db");
  assert.equal(env.FVOCI_BIND, "127.0.0.1:4321");
  assert.equal(env.FVOCI_PUBLIC_ORIGIN, "http://127.0.0.1:4321");
  assert.equal(env.FVOCI_E2E_ADMIN_DATABASE_URL, undefined);
  assert.equal(env.TEST_DATABASE_URL, undefined);
  assert.equal(env.FVOCI_TEST_PG_CONTAINER, undefined);
  assert.equal(env.FVOCI_E2E_SERVER_BIN, undefined);
  assert.equal(env.PEPPER, undefined);
  assert.equal(env.PASSWORD_PEPPER_ACTIVE_KEY_ID, "test");
});

test("process group observation reads this Node process without pid-file daemons", () => {
  const self = readProcMember(process.pid);
  assert.ok(self);
  assert.equal(self?.pid, process.pid);
  assert.ok((self?.starttime ?? "").length > 0);
  assert.ok((self?.pgrp ?? 0) > 0);
  const group = processGroupMembers(self?.pgrp ?? 0);
  assert.equal(
    group.some((member) => member.pid === process.pid && member.starttime === self?.starttime),
    true,
  );
});
