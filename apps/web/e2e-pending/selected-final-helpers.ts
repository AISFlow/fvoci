import { randomUUID } from "node:crypto";
import {
  expect,
  type Browser,
  type BrowserContext,
  type Page,
  type TestInfo,
} from "@playwright/test";
import { yDocToTiptapJson } from "../../../packages/editor/src/collab-tiptap";
import { extractText } from "../../../packages/editor/src/extract";
import type { components } from "../src/generated/api";
import {
  attachCollabWire,
  editorShape,
  expectMatchingPersistAck,
  login,
  persistBody,
  waitConnected,
  sentPersistRequests,
  type CollabWireLog,
} from "./collab-helpers";
import { UUID_RE } from "./collab-wire";
import {
  nativeHistory,
  observeNativeWire,
  documentNodeIds,
  observeSelectedWrites,
} from "./selected-final-native";
import type { FinalBinding } from "./selected-final-contract";

type Schema = components["schemas"];
export type Target = { id: string; workspaceId: string; path: string; url: string };
export type Actor = {
  context: BrowserContext;
  page: Page;
  wire: CollabWireLog;
  native: ReturnType<typeof observeNativeWire>;
  writes: ReturnType<typeof observeSelectedWrites>;
  actorId: string;
  acks: { requestId: string; workspaceId: string; documentId: string; flow: "on" | "off" }[];
};

/** Sessions are private process memory; each readback opens a new context with
 * no origin storage. No actor creation, native fixture or backend startup. */
export class SelectedActors {
  private sessions = new Map<string, Awaited<ReturnType<BrowserContext["cookies"]>>>();
  private contexts: BrowserContext[] = [];
  private observed: Actor[] = [];
  readonly denials: {
    targetId: string;
    workspaceId: string;
    actorId: string;
    method: "PUT";
    status: 404;
    operation: "body-put";
    payloadSha256: string;
  }[] = [];
  constructor(
    readonly browser: Browser,
    readonly baseURL: string,
    readonly binding: FinalBinding,
  ) {}

  async open(name: "owner" | "member" | "outsider", freshSession = false): Promise<Actor> {
    const identity = this.binding[name];
    const context = await this.browser.newContext({ baseURL: this.baseURL });
    this.contexts.push(context);
    const page = await context.newPage();
    const wire = attachCollabWire(page),
      native = observeNativeWire(page);
    const writes = observeSelectedWrites(page, identity.userId);
    const cookies = this.sessions.get(name);
    if (cookies && !freshSession) await context.addCookies(cookies);
    else {
      await login(page, identity.email, identity.password);
      this.sessions.set(name, await context.cookies());
    }
    const response = await page.request.get("/api/v1/auth/me");
    expect(response.status()).toBe(200);
    const user = (await response.json()) as Schema["SessionUserOutput"];
    expect(user.userId).toBe(identity.userId);
    expect(user.email.toLowerCase()).toBe(identity.email.toLowerCase());
    expect(user.sessionId).toMatch(UUID_RE);
    const actor: Actor = {
      context,
      page,
      wire,
      native,
      writes,
      actorId: identity.userId,
      acks: [],
    };
    this.observed.push(actor);
    return actor;
  }

  async verifyAllocation(): Promise<void> {
    const owner = await this.open("owner");
    const setup = await owner.page.request.get("/api/v1/setup");
    expect(setup.status()).toBe(200);
    expect(await setup.json()).toMatchObject({ needed: false });
    for (const name of ["owner", "member", "outsider"] as const) {
      const actor = name === "owner" ? owner : await this.open(name);
      const res = await actor.page.request.get("/api/v1/me/workspaces");
      expect(res.status()).toBe(200);
      const spaces = (await res.json()) as Schema["WorkspaceListResponse"];
      expect(spaces.items.find((w) => w.id === this.binding.workspaceId)).toMatchObject({
        slug: this.binding.workspaceSlug,
        role: name === "owner" ? "owner" : "member",
      });
    }
    await this.close();
  }

  async close() {
    const contexts = this.contexts.splice(0);
    const results = await Promise.allSettled(contexts.map((c) => c.close()));
    if (results.some((r) => r.status === "rejected"))
      throw new Error("SELECTED_FINAL_CONTEXT_CLOSE_FAILED");
    const actors = this.observed
      .splice(0)
      .map((actor) => ({ actorId: actor.actorId, writes: actor.writes(), acks: actor.acks }));
    const denials = this.denials.splice(0);
    return { actors, denials };
  }
}

export async function wiki(page: Page, binding: FinalBinding, title: string): Promise<Target> {
  const path = `/api/v1/workspaces/${binding.workspaceId}/documents`;
  const res = await page.request.post(path, {
    data: { title, commandId: randomUUID(), parentId: null },
  });
  expect(res.status()).toBe(201);
  const document = (await res.json()) as Schema["DocumentMetaResponse"];
  expect(document.id).toMatch(UUID_RE);
  expect(document.workspaceId).toBe(binding.workspaceId);
  return {
    id: document.id,
    workspaceId: binding.workspaceId,
    path: `${path}/${document.id}`,
    url: `/w/${binding.workspaceSlug}/WIKI-${String(document.number)}`,
  };
}

export async function mount(actor: Actor, target: Target, flow: "on" | "off"): Promise<void> {
  const res = await actor.page.goto(target.url);
  expect(res?.status()).toBe(200);
  if (flow === "on") await waitConnected(actor.page);
  else {
    await expect(actor.page.locator('[data-body-mode="off"]')).toBeVisible();
    await expect(actor.page.locator("[data-collab-status]")).toHaveCount(0);
  }
  await expect(
    actor.page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]'),
  ).toBeVisible();
}

export async function body(page: Page, target: Target) {
  const res = await page.request.get(`${target.path}/body`);
  expect(res.status()).toBe(200);
  return (await res.json()) as Schema["BodyResponse"];
}

export async function saveOn(actor: Actor): Promise<string> {
  const previous = sentPersistRequests(actor.wire);
  await persistBody(actor.page);
  const requestId = await expectMatchingPersistAck(actor.page, actor.wire);
  expect(previous).not.toContain(requestId);
  const frame = actor.wire.sent.find(
    (frame) => frame.kind === "stateless" && frame.payload === `persist:${requestId}`,
  );
  if (frame?.kind !== "stateless") throw new Error("SELECTED_FINAL_ACK_TARGET_MISSING");
  const [workspaceId, kind, documentId, extra] = frame.routingKey.split(":");
  if (!workspaceId || !documentId || kind !== "document" || extra)
    throw new Error("SELECTED_FINAL_ACK_TARGET_MISSING");
  actor.acks.push({ requestId, workspaceId, documentId, flow: "on" });
  return requestId;
}

export async function saveOff(actor: Actor, target: Target): Promise<string> {
  const path = `${target.path}/body/versioned`;
  const sent = actor.page.waitForRequest(
    (r) => r.method() === "PUT" && new URL(r.url()).pathname === path,
  );
  const received = actor.page.waitForResponse(
    (r) => r.request().method() === "PUT" && new URL(r.url()).pathname === path,
  );
  await actor.page.getByRole("button", { name: "저장", exact: true }).click();
  const command = (await sent).postDataJSON() as Schema["SaveVersionedBodyInput"];
  expect(command.commandId).toMatch(UUID_RE);
  const response = await received;
  expect(response.status()).toBe(200);
  const ack = (await response.json()) as Schema["SaveVersionedBodyResponse"];
  expect(ack).toMatchObject({ commandId: command.commandId, targetId: target.id });
  expect(ack.revisionId).toMatch(UUID_RE);
  expect(BigInt(ack.tailSeq)).toBeGreaterThan(BigInt(command.expectedTailSeq));
  await expect(actor.page.locator('[data-body-mode="off"]')).toHaveAttribute(
    "data-body-persisted",
    "true",
  );
  expect(actor.wire.sent).toEqual([]);
  actor.acks.push({
    requestId: ack.commandId,
    workspaceId: target.workspaceId,
    documentId: target.id,
    flow: "off",
  });
  return ack.commandId;
}

export async function freshReadback(
  actors: SelectedActors,
  who: "owner" | "member" | "outsider",
  target: Target,
  expected: Awaited<ReturnType<typeof editorShape>>,
  info: TestInfo,
  attachmentBytes?: Buffer,
): Promise<void> {
  const reader = await actors.open(who);
  await mount(reader, target, actors.binding.flow);
  expect(await editorShape(reader.page)).toEqual(expected);
  const current = await body(reader.page, target);
  expect(current.contentJson).toEqual(expected.document);
  if (attachmentBytes) {
    const { storedAttachmentDownloadBytes } = await import("./collab-helpers");
    expect(await storedAttachmentDownloadBytes(reader.page)).toEqual(attachmentBytes);
  }
  const ids = documentNodeIds(current.contentJson);
  expect(ids.length).toBeGreaterThan(0);
  expect(new Set(ids).size).toBe(ids.length);
  for (const id of ids) expect(id).toMatch(UUID_RE);
  if (actors.binding.flow === "on") {
    const native = nativeHistory(
      reader.native,
      `${target.workspaceId}:document:${target.id}`,
      true,
    );
    try {
      expect(yDocToTiptapJson(native)).toEqual(current.contentJson);
    } finally {
      native.destroy();
    }
  } else {
    const res = await reader.page.request.get(`${target.path}/body/versioned`);
    expect(res.status()).toBe(200);
    const native = (await res.json()) as Schema["VersionedBodyResponse"];
    expect(native.targetId).toBe(target.id);
    expect(native.contentJson).toEqual(current.contentJson);
    // OFF native decoding shares the installed Yjs/editor oracle, no JSON reseed.
    const Y = await import("yjs");
    const doc = new Y.Doc({ gc: false });
    try {
      Y.applyUpdate(doc, Buffer.from(native.snapshotV1, "base64"));
      for (const tail of native.tailV1) Y.applyUpdate(doc, Buffer.from(tail, "base64"));
      expect(yDocToTiptapJson(doc)).toEqual(current.contentJson);
    } finally {
      doc.destroy();
    }
  }
  await info.attach("selected-final-native-readback.json", {
    contentType: "application/json",
    body: JSON.stringify({
      schema: 1,
      source: actors.binding.source,
      tree: actors.binding.tree,
      backend: actors.binding.backend,
      browser: actors.binding.browser,
      flow: actors.binding.flow,
      targetId: target.id,
      workspaceId: target.workspaceId,
      ids,
      content: current.contentJson,
      text: extractText(current.contentJson),
      native: "server-received",
      freshContext: true,
    }),
  });
}
