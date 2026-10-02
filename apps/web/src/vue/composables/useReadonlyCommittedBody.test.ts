import { readFileSync } from "node:fs";
import { expect, test } from "bun:test";
import * as Y from "yjs";
import { z } from "zod";
import { installNodeRelativeRequestShim } from "../../../test/node-api-fetch";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { createFvociExtensions } from "@fvoci/editor/tiptap-schema";
import { getSchema } from "@tiptap/core";
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
    kind: "wiki",
    projectId: null,
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
const markedProjection = z
  .object({
    content: z.array(
      z
        .object({
          content: z.array(
            z
              .object({ marks: z.array(z.object({ type: z.string() }).passthrough()) })
              .passthrough(),
          ),
        })
        .passthrough(),
    ),
  })
  .passthrough();
function markedFixture() {
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "marked" },
        content: [
          {
            type: "text",
            text: "한글 meaning",
            marks: [{ type: "bold" }, { type: "textStyle", attrs: { color: "#112233" } }],
          },
        ],
      },
    ],
  });
  const live = yDocToTiptapJson(doc);
  const stored = markedProjection.parse(structuredClone(live));
  const marks = stored.content[0]?.content[0]?.marks;
  if (!marks) throw new Error("missing actual marks");
  // Pinned Rust projector sorts raw unique mark keys; this expected order
  // is independently established from project.rs, not the helper under test.
  marks.sort((a, b) => (a.type < b.type ? -1 : a.type > b.type ? 1 : 0));
  const base = fixture();
  base.doc.destroy();
  return { scope: { ...base, doc, schema: getSchema(createFvociExtensions()) }, live, stored };
}
test("actual JS marked run matches documented Rust unique-mark projection order without losing attrs", async () => {
  const f = markedFixture();
  try {
    expect(
      markedProjection.parse(f.live).content[0]?.content[0]?.marks.map((mark) => mark.type),
    ).toEqual(["textStyle", "bold"]);
    expect(f.stored.content[0]?.content[0]?.marks.map((mark) => mark.type)).toEqual([
      "bold",
      "textStyle",
    ]);
    expect(
      await useReadonlyCommittedBody(
        () => f.scope,
        () => Promise.resolve(f.stored),
      )(),
    ).toBe(true);
  } finally {
    f.scope.doc.destroy();
  }
});
test("actual SDK null-prototype mark attrs match JSON-parsed own data with unchanged mark order", async () => {
  const f = markedFixture();
  try {
    const raw = markedProjection
      .parse(f.live)
      .content[0]?.content[0]?.marks.find((mark) => mark.type === "textStyle");
    if (!raw) throw new Error("missing style");
    // Read the actual projection rather than the parser's copied attrs.
    const json = z
      .object({
        content: z.array(
          z.object({
            content: z.array(
              z.object({ marks: z.array(z.object({ type: z.string(), attrs: z.unknown() })) }),
            ),
          }),
        ),
      })
      .parse(f.live);
    expect(Object.getPrototypeOf(json.content[0]?.content[0]?.marks[0]?.attrs)).toBeNull();
    const transported: unknown = JSON.parse(JSON.stringify(f.live));
    expect(
      await useReadonlyCommittedBody(
        () => f.scope,
        () => Promise.resolve(transported),
      )(),
    ).toBe(true);
  } finally {
    f.scope.doc.destroy();
  }
});
for (const change of [
  "missingMark",
  "changedType",
  "changedAttr",
  "missingAttrs",
  "undefinedAttrs",
  "nullAttrs",
  "emptyAttrs",
  "id",
  "duplicate",
  "opaqueOrder",
  "getter",
  "customPrototype",
] as const) {
  test(`supported-mark transport comparison rejects ${change} without stripping own data`, async () => {
    const f = markedFixture();
    let getterCalls = 0;
    try {
      const node = f.stored.content[0],
        text = node?.content[0];
      const style = text?.marks.find((mark) => mark.type === "textStyle");
      if (!node || !text || !style) throw new Error("missing marked fixture");
      if (change === "missingMark") text.marks.shift();
      if (change === "changedType") style.type = "futureMark";
      if (change === "changedAttr") style.attrs = { color: "#ffffff" };
      if (change === "missingAttrs") delete style.attrs;
      if (change === "undefinedAttrs") style.attrs = undefined;
      if (change === "nullAttrs") style.attrs = null;
      if (change === "emptyAttrs") style.attrs = {};
      if (change === "id") node.attrs = { id: "changed-id" };
      if (change === "duplicate") text.marks.push({ ...style, attrs: { color: "#ffffff" } });
      if (change === "opaqueOrder") {
        const actual = f.scope.doc.getXmlFragment("prosemirror").get(0);
        if (!(actual instanceof Y.XmlElement)) throw new Error("missing actual node");
        actual.setAttribute("opaque", { order: ["a", "b"], reference: "stable-ref" });
        node.attrs = { id: "marked", opaque: { order: ["b", "a"], reference: "stable-ref" } };
      }
      if (change === "getter") {
        const attrs: Record<string, unknown> = {};
        Object.defineProperty(attrs, "color", {
          enumerable: true,
          get: () => {
            getterCalls++;
            return "#112233";
          },
        });
        style.attrs = attrs;
      }
      if (change === "customPrototype")
        style.attrs = Object.assign(Object.create({ opaque: true }) as object, {
          color: "#112233",
        });
      expect(
        await useReadonlyCommittedBody(
          () => f.scope,
          () => Promise.resolve(f.stored),
        )(),
      ).toBe(false);
      expect(getterCalls).toBe(0);
    } finally {
      f.scope.doc.destroy();
    }
  });
}
test("own __proto__ mark data is compared as data and explicit undefined is never absence/null", async () => {
  const f = markedFixture();
  try {
    const paragraph = f.scope.doc.getXmlFragment("prosemirror").get(0);
    if (!(paragraph instanceof Y.XmlElement)) throw new Error("missing paragraph");
    const text = paragraph.get(0);
    if (!(text instanceof Y.XmlText)) throw new Error("missing actual styled run");
    const attrs: Record<string, unknown> = { color: "#112233" };
    Object.defineProperty(attrs, "__proto__", { value: "opaque own data", enumerable: true });
    text.format(0, text.length, { textStyle: attrs });
    const live = yDocToTiptapJson(f.scope.doc);
    const transported: unknown = JSON.parse(JSON.stringify(live));
    expect(
      await useReadonlyCommittedBody(
        () => f.scope,
        () => Promise.resolve(transported),
      )(),
    ).toBe(true);
    for (const shape of ["absent", "undefined", "null"] as const) {
      const stored = markedProjection.parse(structuredClone(live));
      const style = stored.content[0]?.content[0]?.marks.find((mark) => mark.type === "textStyle");
      if (!style) throw new Error("missing style");
      style.attrs = { color: "#112233" };
      if (shape !== "absent")
        Object.defineProperty(style.attrs, "__proto__", {
          value: shape === "undefined" ? undefined : null,
          enumerable: true,
        });
      expect(
        await useReadonlyCommittedBody(
          () => f.scope,
          () => Promise.resolve(stored),
        )(),
      ).toBe(false);
    }
  } finally {
    f.scope.doc.destroy();
  }
});
for (const opaque of ["unknownMark", "duplicateType", "unknownNode"] as const) {
  test(`raw ${opaque} ordering stays opaque even when supported mark attrs can be transported`, async () => {
    const f = markedFixture();
    try {
      const fragment = f.scope.doc.getXmlFragment("prosemirror");
      fragment.delete(0, fragment.length);
      const paragraph = new Y.XmlElement(opaque === "unknownNode" ? "futureNode" : "paragraph");
      paragraph.setAttribute("id", "opaque");
      const text = new Y.XmlText();
      paragraph.push([text]);
      fragment.push([paragraph]);
      text.insert(
        0,
        "opaque",
        opaque === "duplicateType"
          ? { "bold--abcdefgh": { value: "a" }, "bold--ijklmnop": { value: "b" } }
          : opaque === "unknownMark"
            ? { futureMark: { opaque: "keep" }, bold: {} }
            : { textStyle: { color: "#112233" }, bold: {} },
      );
      const stored = markedProjection.parse(structuredClone(yDocToTiptapJson(f.scope.doc)));
      const marks = stored.content[0]?.content[0]?.marks;
      if (!marks) throw new Error("missing opaque marks");
      marks.reverse();
      expect(
        await useReadonlyCommittedBody(
          () => f.scope,
          () => Promise.resolve(stored),
        )(),
      ).toBe(false);
    } finally {
      f.scope.doc.destroy();
    }
  });
}
test("supported content order is exact while mark order alone may differ", async () => {
  const f = markedFixture();
  try {
    const tail = new Y.XmlElement("paragraph");
    tail.setAttribute("id", "tail-id");
    const text = new Y.XmlText("unrelated tail");
    tail.push([text]);
    f.scope.doc.getXmlFragment("prosemirror").push([tail]);
    const stored = z
      .object({ content: z.array(z.unknown()) })
      .passthrough()
      .parse(structuredClone(yDocToTiptapJson(f.scope.doc)));
    stored.content.reverse();
    expect(
      await useReadonlyCommittedBody(
        () => f.scope,
        () => Promise.resolve(stored),
      )(),
    ).toBe(false);
  } finally {
    f.scope.doc.destroy();
  }
});
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

test("DEFAULT generated-client wiki read awaits actual ensureOk and compares stored JSON", async () => {
  const scope = fixture();
  const restore = installNodeRelativeRequestShim();
  const previous = globalThis.fetch;
  const requests: Request[] = [];
  globalThis.fetch = (input: RequestInfo | URL) => {
    if (!(input instanceof Request)) throw new Error("default client did not construct a Request");
    requests.push(input);
    return Promise.resolve(Response.json({ contentJson: expected, version: 1 }));
  };
  try {
    const result = await useReadonlyCommittedBody(() => scope)();
    expect(requests.map((request) => new URL(request.url).pathname)).toEqual([
      "/api/v1/workspaces/ws/documents/source/body",
    ]);
    expect(requests[0]?.cache).toBe("no-store");
    expect(result).toBe(true);
  } finally {
    globalThis.fetch = previous;
    restore();
    scope.doc.destroy();
  }
});
test("DEFAULT generated-client project read uses the authorized project body endpoint", async () => {
  const scope: ReadonlyBodyScope = { ...fixture(), kind: "project", projectId: "project" };
  const restore = installNodeRelativeRequestShim();
  const previous = globalThis.fetch;
  const paths: string[] = [];
  globalThis.fetch = (input: RequestInfo | URL) => {
    if (!(input instanceof Request)) throw new Error("missing actual request");
    paths.push(new URL(input.url).pathname);
    return Promise.resolve(Response.json({ contentJson: expected, version: 1 }));
  };
  try {
    const result = await useReadonlyCommittedBody(() => scope)();
    expect(paths).toEqual(["/api/v1/workspaces/ws/projects/project/documents/source/body"]);
    expect(result).toBe(true);
  } finally {
    globalThis.fetch = previous;
    restore();
    scope.doc.destroy();
  }
});
test("DEFAULT generated-client task read uses GET task contentJson", async () => {
  const scope: ReadonlyBodyScope = { ...fixture(), kind: "task" };
  const restore = installNodeRelativeRequestShim();
  const previous = globalThis.fetch;
  const paths: string[] = [];
  globalThis.fetch = (input: RequestInfo | URL) => {
    if (!(input instanceof Request)) throw new Error("missing actual request");
    paths.push(new URL(input.url).pathname);
    return Promise.resolve(Response.json({ contentJson: expected }));
  };
  try {
    expect(await useReadonlyCommittedBody(() => scope)()).toBe(true);
    expect(paths).toEqual(["/api/v1/workspaces/ws/tasks/source"]);
  } finally {
    globalThis.fetch = previous;
    restore();
    scope.doc.destroy();
  }
});
for (const status of [401, 403, 404]) {
  test(`DEFAULT project ${String(status)} rejects once without wiki fallback or unhandled ensureOk rejection`, async () => {
    const scope: ReadonlyBodyScope = { ...fixture(), kind: "project", projectId: "project" };
    const restore = installNodeRelativeRequestShim();
    const previous = globalThis.fetch;
    const paths: string[] = [];
    globalThis.fetch = (input: RequestInfo | URL) => {
      if (!(input instanceof Request)) throw new Error("missing actual request");
      paths.push(new URL(input.url).pathname);
      return Promise.resolve(Response.json({ code: "not_found" }, { status }));
    };
    try {
      expect(await useReadonlyCommittedBody(() => scope)()).toBe(false);
      expect(paths).toEqual(["/api/v1/workspaces/ws/projects/project/documents/source/body"]);
      expect(scope.doc._observers.get("update")?.size ?? 0).toBe(0);
    } finally {
      globalThis.fetch = previous;
      restore();
      scope.doc.destroy();
    }
  });
}
for (const changed of ["project", "document", "actor", "generation"] as const) {
  test(`DEFAULT project late response cannot settle ${changed} ABA scope`, async () => {
    const scope: ReadonlyBodyScope = { ...fixture(), kind: "project", projectId: "project" };
    let active = { ...scope };
    const restore = installNodeRelativeRequestShim();
    const previous = globalThis.fetch;
    const paths: string[] = [];
    let resolve: (response: Response) => void = () => {
      throw new Error("unbound transport");
    };
    const response = new Promise<Response>((done) => {
      resolve = done;
    });
    globalThis.fetch = (input: RequestInfo | URL) => {
      if (!(input instanceof Request)) throw new Error("missing actual request");
      paths.push(new URL(input.url).pathname);
      return response;
    };
    try {
      const pending = useReadonlyCommittedBody(() => active)();
      expect(paths).toEqual(["/api/v1/workspaces/ws/projects/project/documents/source/body"]);
      if (changed === "project") active = { ...active, projectId: "other", lifetime: 2 };
      if (changed === "document") active = { ...active, targetId: "other", lifetime: 2 };
      if (changed === "actor")
        active = { ...active, actorId: "other", credentialId: "other-session", lifetime: 2 };
      if (changed === "generation") active = { ...active, generation: 2, lifetime: 2 };
      active = { ...scope, lifetime: 3 };
      resolve(Response.json({ contentJson: expected, version: 1 }));
      expect(await pending).toBe(false);
    } finally {
      globalThis.fetch = previous;
      restore();
      scope.doc.destroy();
    }
  });
}
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
