import assert from "node:assert/strict";
import test from "node:test";
import { ArchiveLifetime, canConfirmNativeArchive } from "./native-archive";

await test("workspace, permission or session reset aborts old capture and rejects late publication", () => {
  const lifetime = new ArchiveLifetime();
  const source = lifetime.capture("source", "author", "session-a");
  lifetime.reset();
  const destination = lifetime.capture("destination", "author", "session-b");
  assert.equal(source.signal.aborted, true);
  assert.equal(source.current(), false);
  assert.equal(destination.current(), true);
  lifetime.reset();
  assert.equal(destination.current(), false);
});

await test("confirmation requires complete scope, exact destination actor and immutable hash", () => {
  const preflight = {
    complete: true,
    archiveHash: "a".repeat(64),
    destinationWorkspaceId: "private",
    destinationActorId: "actual-actor",
    preservedContentIds: true,
    requiresCollisionFreeInstallation: true,
    diagnostics: [],
  };
  assert.equal(canConfirmNativeArchive(preflight, "private", "actual-actor"), true);
  assert.equal(canConfirmNativeArchive(preflight, "foreign", "actual-actor"), false);
  assert.equal(canConfirmNativeArchive(preflight, "private", "foreign-actor"), false);
  for (const change of [
    { complete: false },
    { archiveHash: "wrong" },
    { preservedContentIds: false },
    { diagnostics: ["comments unsupported"] },
  ]) {
    assert.equal(
      canConfirmNativeArchive({ ...preflight, ...change }, "private", "actual-actor"),
      false,
    );
  }
});
