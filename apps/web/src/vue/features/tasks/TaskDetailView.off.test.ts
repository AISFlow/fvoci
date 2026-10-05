import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import ts from "typescript";

const text = readFileSync(new URL("./TaskDetailView.vue", import.meta.url), "utf8")
  .split('<script setup lang="ts">')[1]!
  .split("</script>")[0]!;
const source = ts.createSourceFile("task.ts", text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
const functions = source.statements
  .filter(
    (node) =>
      ts.isFunctionDeclaration(node) &&
      ["persistOffBody", "handleArchiveToggle"].includes(node.name?.text ?? ""),
  )
  .map((node) => node.getText(source))
  .join("\n");
const script = ts.transpile(functions, {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.None,
});
function deferred() {
  let resolve!: (value: boolean) => void;
  const promise = new Promise<boolean>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function harness() {
  const draft = { active: true };
  const save = deferred(),
    verify = deferred(),
    verifyStarted = deferred();
  const order: string[] = [];
  const offBody = {
    draft: { value: draft },
    save: () => {
      order.push("save");
      return save.promise;
    },
    verifyCommitted: () => {
      order.push("verify");
      verifyStarted.resolve(true);
      return verify.promise;
    },
  };
  const archivePersistError = { value: null as string | null };
  const factory = runInNewContext(
    `${script}\n({ handleArchiveToggle, persistOffBody, retire: () => hostGeneration++ })`,
    {
      hostGeneration: 1,
      offBody,
      realtimeOff: { value: true },
      authRetired: { value: false },
      archiveInFlight: false,
      archivePersisting: { value: false },
      archivePersistError,
      pageEditable: { value: true },
      props: {
        archivePending: false,
        onArchiveToggle: async () => {
          order.push("archive");
        },
      },
      // OFF must reach the actual HTTP body barrier, never a fabricated session.
      runArchiveWithBodyPersist: () => {
        throw new Error("OFF borrowed ON persist");
      },
      t: (key: string) => key,
    },
  );
  return { factory, offBody, save, verify, verifyStarted, order, archivePersistError };
}
test("actual OFF task archive waits for confirmed body save and fresh native readback", async () => {
  const h = harness();
  const archiving = h.factory.handleArchiveToggle(true);
  expect(h.order).toEqual(["save"]);
  h.save.resolve(true);
  await h.verifyStarted.promise;
  expect(h.order).toEqual(["save", "verify"]);
  h.verify.resolve(true);
  await archiving;
  expect(h.order).toEqual(["save", "verify", "archive"]);
  expect(h.archivePersistError.value).toBeNull();
});
for (const boundary of ["save", "verify", "retired", "replaced"]) {
  test(`OFF archive refuses ${boundary} without metadata effect`, async () => {
    const h = harness();
    const archiving = h.factory.handleArchiveToggle(true);
    h.save.resolve(boundary !== "save");
    if (boundary !== "save") await h.verifyStarted.promise;
    if (boundary === "retired") h.factory.retire();
    if (boundary === "replaced") h.offBody.draft.value = { active: true };
    h.verify.resolve(boundary !== "verify");
    await archiving;
    expect(h.order).not.toContain("archive");
    expect(h.archivePersistError.value).toBe("task.archive.persistFailed");
  });
}
