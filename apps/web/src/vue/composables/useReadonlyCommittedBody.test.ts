import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import * as Y from "yjs";
import { z } from "zod";
import { useReadonlyCommittedBody, type ReadonlyBodyScope } from "./useReadonlyCommittedBody";

const fixtureRoot = new URL("../../../../../crates/collab-engine/fixtures/", import.meta.url);
const expected = z
  .object({ structured: z.object({ prosemirror_json: z.unknown() }) })
  .parse(JSON.parse(readFileSync(new URL("expectations.json", fixtureRoot), "utf8")))
  .structured.prosemirror_json;
function fixture() {
  const doc = new Y.Doc({ gc: false });
  Y.applyUpdate(doc, readFileSync(new URL("structured.v1", fixtureRoot)));
  const scope: ReadonlyBodyScope = {
    workspaceId: "ws",
    targetId: "source",
    kind: "document",
    actorId: "actor",
    credentialId: "session",
    lifetime: 1,
    doc,
    provider: {},
    generation: 1,
    connected: true,
    synced: true,
    pending: false,
    allowed: true,
  };
  return scope;
}
function deferred() {
  let resolve: (value: unknown) => void = () => {
    throw new Error("unbound read");
  };
  const promise = new Promise<unknown>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
test("fresh readonly structured fixture matches independent stored JSON with no saved-flag promotion", async () => {
  const scope = fixture();
  try {
    const seen: ReadonlyBodyScope[] = [];
    const verify = useReadonlyCommittedBody(
      () => scope,
      (target) => {
        seen.push(target);
        return Promise.resolve(expected);
      },
    );
    expect(await verify()).toBe(true);
    expect(seen).toEqual([scope]);
    expect(Object.hasOwn(scope, "durableSaved")).toBe(false);
  } finally {
    scope.doc.destroy();
  }
});
test("local uncommitted body cannot pass a synced readonly copy barrier", async () => {
  const scope = fixture();
  try {
    scope.doc.getXmlFragment("prosemirror").delete(0, 1);
    expect(
      await useReadonlyCommittedBody(
        () => scope,
        () => Promise.resolve(expected),
      )(),
    ).toBe(false);
  } finally {
    scope.doc.destroy();
  }
});
for (const mutation of ["peer", "deletion"] as const) {
  test(`${mutation} during the authorized read invalidates its captured content epoch`, async () => {
    const scope = fixture();
    const read = deferred();
    try {
      const verify = useReadonlyCommittedBody(
        () => scope,
        () => read.promise,
      );
      const pending = verify();
      if (mutation === "deletion") scope.doc.getXmlFragment("prosemirror").delete(0, 1);
      else {
        const text = new Y.XmlText("changed peer");
        scope.doc.getXmlFragment("prosemirror").push([text]);
      }
      read.resolve(expected);
      expect(await pending).toBe(false);
    } finally {
      scope.doc.destroy();
    }
  });
}
for (const retired of ["targetABA", "actorABA", "sessionABA", "provider", "revocation"] as const) {
  test(`late readonly response after ${retired} cannot publish`, async () => {
    const scope = fixture();
    const read = deferred();
    let active = { ...scope };
    try {
      const pending = useReadonlyCommittedBody(
        () => active,
        () => read.promise,
      )();
      if (retired === "targetABA") active = { ...active, lifetime: active.lifetime + 2 };
      if (retired === "actorABA") active = { ...active, lifetime: active.lifetime + 2 };
      if (retired === "sessionABA")
        active = { ...active, credentialId: "new-session", lifetime: active.lifetime + 2 };
      if (retired === "provider") active = { ...active, provider: {}, generation: 2 };
      if (retired === "revocation") active = { ...active, allowed: false };
      read.resolve(expected);
      expect(await pending).toBe(false);
    } finally {
      scope.doc.destroy();
    }
  });
}
test("authorization errors and retirement deny copying and remove temporary observers", async () => {
  const scope = fixture();
  let active: ReadonlyBodyScope | null = scope;
  const read = deferred();
  try {
    const pending = useReadonlyCommittedBody(
      () => active,
      () => read.promise,
    )();
    active = null;
    read.resolve(expected);
    expect(await pending).toBe(false);
    for (const status of [401, 403])
      expect(
        await useReadonlyCommittedBody(
          () => scope,
          () => Promise.reject(new Error(String(status))),
        )(),
      ).toBe(false);
    expect(scope.doc._observers.get("update")?.size ?? 0).toBe(0);
  } finally {
    scope.doc.destroy();
  }
});
