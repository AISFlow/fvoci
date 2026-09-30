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

await test("runArchiveWithBodyPersist calls archive only after persist succeeds", async () => {
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
    archive: () => {
      order.push("archive");

      return Promise.resolve();
    },
  });
  assert.deepEqual(order, ["persist-start"]);
  releasePersist();
  await flow;
  assert.deepEqual(order, ["persist-start", "persist-end", "archive"]);
});

await test("failed persist prevents archive PATCH", async () => {
  let archived = false;
  await assert.rejects(
    runArchiveWithBodyPersist({
      pageEditable: true,
      session: fakeSession({
        persistNow: () => {
          return Promise.reject(new Error("collab unavailable"));
        },
      }),
      collabUser,
      archive: () => {
        archived = true;

        return Promise.resolve();
      },
    }),
    /collab unavailable/,
  );
  assert.equal(archived, false);
});

await test("retry after failed persist can reach archive", async () => {
  let fails = true;
  let archived = false;
  await assert.rejects(
    runArchiveWithBodyPersist({
      pageEditable: true,
      session: fakeSession({
        persistNow: () => {
          if (fails) return Promise.reject(new Error("collab unavailable"));

          return Promise.resolve();
        },
      }),
      collabUser,
      archive: () => {
        archived = true;

        return Promise.resolve();
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
    archive: () => {
      archived = true;

      return Promise.resolve();
    },
  });
  assert.equal(archived, true);
});

await test("read-only page skips persist but still archives", async () => {
  let persisted = false;
  let archived = false;
  await runArchiveWithBodyPersist({
    pageEditable: false,
    session: fakeSession({
      persistNow: () => {
        persisted = true;

        return Promise.resolve();
      },
    }),
    collabUser,
    archive: () => {
      archived = true;

      return Promise.resolve();
    },
  });
  assert.equal(persisted, false);
  assert.equal(archived, true);
});

await test("never-synced body room skips persist (still connecting)", async () => {
  let persisted = false;
  await runArchiveWithBodyPersist({
    pageEditable: true,
    session: fakeSession({
      synced: false,
      persistNow: () => {
        persisted = true;

        return Promise.resolve();
      },
    }),
    collabUser,
    archive: async () => {},
  });
  assert.equal(persisted, false);
});

await test("disconnected after initial sync blocks archive", async () => {
  const archived = false;
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
