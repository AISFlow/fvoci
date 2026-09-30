import assert from "node:assert/strict";
import test from "node:test";
import type { CollabSession, CollabUser } from "@/features/documents/collab-model";
import { persistTaskBodyBeforeArchive, runArchiveWithBodyPersist } from "./task-archive-persist.ts";

const collabUser: CollabUser = { id: "u1", name: "Tester", color: "#000" };

function fakeSession(overrides: Partial<CollabSession> = {}): CollabSession {
  return {
    provider: {} as CollabSession["provider"],
    doc: {} as CollabSession["doc"],
    fragment: {} as CollabSession["fragment"],
    status: "connected",
    synced: true,
    pending: false,
    durableSaved: true,
    peers: [],
    readOnly: false,
    persistNow: async () => {},
    ...overrides,
  };
}

test("runArchiveWithBodyPersist calls archive only after persist succeeds", async () => {
  const order: string[] = [];
  let releasePersist!: () => void;
  const persistGate = new Promise<void>((resolve) => {
    releasePersist = resolve;
  });
  const flow = runArchiveWithBodyPersist({
    pageEditable: true,
    session: fakeSession({
      persistNow: async () => {
        order.push("persist-start");
        await persistGate;
        order.push("persist-end");
      },
    }),
    collabUser,
    archive: async () => {
      order.push("archive");
    },
  });
  assert.deepEqual(order, ["persist-start"]);
  releasePersist();
  await flow;
  assert.deepEqual(order, ["persist-start", "persist-end", "archive"]);
});

test("failed persist prevents archive PATCH", async () => {
  let archived = false;
  await assert.rejects(
    runArchiveWithBodyPersist({
      pageEditable: true,
      session: fakeSession({
        persistNow: async () => {
          throw new Error("collab unavailable");
        },
      }),
      collabUser,
      archive: async () => {
        archived = true;
      },
    }),
    /collab unavailable/,
  );
  assert.equal(archived, false);
});

test("retry after failed persist can reach archive", async () => {
  let fails = true;
  let archived = false;
  await assert.rejects(
    runArchiveWithBodyPersist({
      pageEditable: true,
      session: fakeSession({
        persistNow: async () => {
          if (fails) throw new Error("collab unavailable");
        },
      }),
      collabUser,
      archive: async () => {
        archived = true;
      },
    }),
    /collab unavailable/,
  );
  fails = false;
  await runArchiveWithBodyPersist({
    pageEditable: true,
    session: fakeSession({
      persistNow: async () => {},
    }),
    collabUser,
    archive: async () => {
      archived = true;
    },
  });
  assert.equal(archived, true);
});

test("read-only page skips persist but still archives", async () => {
  let persisted = false;
  let archived = false;
  await runArchiveWithBodyPersist({
    pageEditable: false,
    session: fakeSession({
      persistNow: async () => {
        persisted = true;
      },
    }),
    collabUser,
    archive: async () => {
      archived = true;
    },
  });
  assert.equal(persisted, false);
  assert.equal(archived, true);
});

test("never-synced body room skips persist (still connecting)", async () => {
  let persisted = false;
  await runArchiveWithBodyPersist({
    pageEditable: true,
    session: fakeSession({ synced: false, persistNow: async () => { persisted = true; } }),
    collabUser,
    archive: async () => {},
  });
  assert.equal(persisted, false);
});

test("disconnected after initial sync blocks archive", async () => {
  let archived = false;
  await assert.rejects(
    persistTaskBodyBeforeArchive({
      pageEditable: true,
      session: fakeSession({ synced: true, status: "disconnected" }),
      collabUser,
    }),
    /disconnected/,
  );
  assert.equal(archived, false);
});
