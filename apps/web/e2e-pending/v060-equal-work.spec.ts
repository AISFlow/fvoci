import { existsSync, linkSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page, type WebSocket as BrowserSocket } from "@playwright/test";
import * as Y from "yjs";
import * as decoding from "lib0/decoding";
import * as encoding from "lib0/encoding";
import * as sync from "y-protocols/sync";
import { yDocToTiptapJson } from "../../../packages/editor/src/collab-tiptap";
import { extractText } from "../../../packages/editor/src/extract";
import {
  equalTitle,
  phases,
  workload,
  workloadHash,
  type ConnectionFacts,
} from "../../../tools/selected-backend-ci/drivers/equal-work-policy";
import type { components } from "../src/generated/api";
import {
  admin,
  member,
  attachCollabWire,
  editorShape,
  ensureInstanceSetup,
  expectMatchingPersistAck,
  expectNotDurablySaved,
  login,
  newCollabContext,
  persistBody,
  waitConnected,
  type CollabWireLog,
} from "./collab-helpers";
import { decodeHocuspocusFrame, frameBytes, UUID_RE } from "./collab-wire";
import {
  installSelectedMember,
  requiredFixtureInput,
  selectedSetupNeeded,
} from "./selected-backend-fixture";

type Schema = components["schemas"];
// Reuse the wire helper's endpoint classification. Keep total observations
// separate from collaboration connections, and use the SDK's live state.
function observeConnections(page: Page): () => ConnectionFacts {
  const sockets = new Map<BrowserSocket, boolean>();
  let closeEvents = 0,
    contextClosed = false;
  page.context().on("close", () => {
    contextClosed = true;
  });
  page.on("websocket", (socket) => {
    sockets.set(socket, socket.url().includes("/collab"));
    socket.on("close", () => {
      closeEvents++;
    });
  });
  return () => {
    // The positive context close event retires every connection it owned,
    // including objects whose individual close event was not dispatched.
    const active = contextClosed ? [] : [...sockets.keys()].filter((s) => !s.isClosed());
    return {
      opened: sockets.size,
      collabOpened: [...sockets.values()].filter(Boolean).length,
      closeEvents,
      active: active.length,
      collabActive: active.filter((s) => sockets.get(s)).length,
      contextClosed,
    };
  };
}
test(equalTitle, async ({ browser, baseURL }, info) => {
  const backend = requiredFixtureInput("FVOCI_E2E_SELECTED_BACKEND");
  const flow = requiredFixtureInput("FVOCI_E2E_SELECTED_FLOW");
  expect(["on", "off"]).toContain(flow);
  if (!baseURL) throw new Error("allocated base is required");
  const directory = requiredFixtureInput("FVOCI_E2E_EQUAL_WORK_DIR");
  let phase = 0;
  const marker = async (name: string) => {
    expect(name).toBe(phases[phase]);
    const prefix = join(directory, String(phase));
    writeFileSync(prefix + ".pending", JSON.stringify({ phase: name, workloadHash }), {
      flag: "wx",
      mode: 0o600,
    });
    linkSync(prefix + ".pending", prefix + ".request.json");
    unlinkSync(prefix + ".pending");
    await expect
      .poll(() => {
        if (existsSync(join(directory, "failed.json")))
          throw new Error("resource collector failed");
        return existsSync(prefix + ".reply.json");
      })
      .toBe(true);
    expect(JSON.parse(readFileSync(prefix + ".reply.json", "utf8"))).toEqual({
      phase: name,
      workloadHash,
    });
    phase++;
  };
  const context = await newCollabContext(browser, baseURL);
  const page = await context.newPage();
  const wire = attachCollabWire(page);
  const ackIds: string[] = [],
    ackLatencyMs: number[] = [];
  const ownerConnections = observeConnections(page);
  const resources: {
    phase: string;
    ownerConnections: ConnectionFacts;
    observedRoomKeys: number;
  }[] = [];
  const freshConnections: {
    text: string;
    beforeClose: ConnectionFacts;
    afterClose: ConnectionFacts;
  }[] = [];
  let ownerClosed = false;
  const mount = async (p: Page, url: string, navigate = true) => {
    if (navigate) expect((await p.goto(url))?.status()).toBe(200);
    else await expect(p).toHaveURL(url);
    if (flow === "on") await waitConnected(p);
    else {
      await expect(p.locator('[data-body-mode="off"]')).toBeVisible();
      await expect(p.locator("[data-collab-status]")).toHaveCount(0);
    }
    await expect(p.locator('.fvoci-editor .ProseMirror[contenteditable="true"]')).toBeVisible();
  };
  try {
    expect((await page.request.get("/api/v1/setup")).status()).toBe(200);
    expect(selectedSetupNeeded()).toBe(true);
    await ensureInstanceSetup(page);
    await login(page, admin.email, admin.password);
    const ownerResponse = await page.request.get("/api/v1/auth/me");
    expect(ownerResponse.status()).toBe(200);
    const owner = (await ownerResponse.json()) as Schema["SessionUserOutput"];
    const workspaces = await page.request.get("/api/v1/me/workspaces");
    expect(workspaces.status()).toBe(200);
    const workspace = ((await workspaces.json()) as Schema["WorkspaceListResponse"]).items.find(
      (w) => w.slug === admin.workspaceSlug,
    );
    if (!workspace) throw new Error("setup workspace missing");
    expect(workspace.role).toBe("owner");
    const documents = `/api/v1/workspaces/${workspace.id}/documents`;
    await page.goto(`/w/${admin.workspaceSlug}/wiki`);
    const created = page.waitForResponse(
      (r) => r.request().method() === "POST" && new URL(r.url()).pathname === documents,
    );
    await page.getByRole("button", { name: "새 문서", exact: true }).click();
    const response = await created;
    expect(response.status()).toBe(201);
    const document = (await response.json()) as Schema["DocumentMetaResponse"];
    expect(document.id).toMatch(UUID_RE);
    expect(document.createdBy).toBe(owner.userId);
    expect(document.workspaceId).toBe(workspace.id);
    const path = `${documents}/${document.id}`;
    if (typeof document.displayId !== "string") throw new Error("created wiki display ID missing");
    const url = `/w/${admin.workspaceSlug}/${document.displayId}`;
    installSelectedMember(backend);
    // The real Create command already routes to this document. A second full
    // navigation creates a second transport before the first observer retires.
    await mount(page, url, false);
    expect((await editorShape(page)).text).toBe("");
    const readback = async (text: string) => {
      const fresh = await newCollabContext(browser, baseURL);
      let observed: (() => ConnectionFacts) | undefined;
      let beforeClose: ConnectionFacts | undefined;
      try {
        const reader = await fresh.newPage();
        observed = observeConnections(reader);
        const receivedNative: Uint8Array[] = [];
        reader.on("websocket", (socket) =>
          socket.on("framereceived", ({ payload }) => {
            const bytes = frameBytes(payload),
              frame = decodeHocuspocusFrame(bytes);
            if (frame?.kind === "other" && frame.type === 0)
              receivedNative.push(Uint8Array.from(bytes));
          }),
        );
        await login(reader, member.email, member.password);
        const identity = await reader.request.get("/api/v1/auth/me");
        expect(identity.status()).toBe(200);
        const user = (await identity.json()) as Schema["SessionUserOutput"];
        expect(user.userId).not.toBe(owner.userId);
        expect(user.sessionId).not.toBe(owner.sessionId);
        await mount(reader, url);
        await expect.poll(async () => (await editorShape(reader)).text).toBe(text);
        const meta = await reader.request.get(path);
        expect(meta.status()).toBe(200);
        expect(await meta.json()).toMatchObject({ id: document.id, workspaceId: workspace.id });
        const bodyResponse = await reader.request.get(
          `${path}/body${flow === "off" ? "/versioned" : ""}`,
        );
        expect(bodyResponse.status()).toBe(200);
        const body = (await bodyResponse.json()) as Schema["VersionedBodyResponse"] &
          Schema["BodyResponse"];
        expect(extractText(body.contentJson)).toBe(text);
        const native = new Y.Doc({ gc: false });
        try {
          if (flow === "off") {
            expect(body.targetId).toBe(document.id);
            expect(body.writable).toBe(true);
            Y.applyUpdate(native, Buffer.from(body.snapshotV1, "base64"));
            for (const update of body.tailV1) Y.applyUpdate(native, Buffer.from(update, "base64"));
          } else {
            expect(body.version).toBeGreaterThan(0);
            let fullSync = 0;
            for (const bytes of receivedNative) {
              const decoder = decoding.createDecoder(bytes);
              expect(decoding.readVarString(decoder)).toBe(
                `${workspace.id}:document:${document.id}`,
              );
              expect(decoding.readVarUint(decoder)).toBe(0);
              if (
                sync.readSyncMessage(decoder, encoding.createEncoder(), native, null) ===
                sync.messageYjsSyncStep2
              )
                fullSync++;
              expect(decoder.pos).toBe(bytes.length);
            }
            expect(fullSync).toBeGreaterThan(0);
          }
          expect(yDocToTiptapJson(native)).toEqual(body.contentJson);
        } finally {
          native.destroy();
        }
        beforeClose = observed();
        await info.attach(`fresh-connections-${String(freshConnections.length)}.json`, {
          contentType: "application/json",
          body: JSON.stringify(beforeClose),
        });
        expect(beforeClose.collabActive).toBe(flow === "on" ? 1 : 0);
      } finally {
        await fresh.close();
      }
      if (!observed || !beforeClose) throw new Error("fresh connection observation missing");
      const afterClose = observed();
      expect(afterClose.contextClosed).toBe(true);
      expect(afterClose.active).toBe(0);
      freshConnections.push({ text, beforeClose, afterClose });
    };
    const editSave = async (text: string, observed: CollabWireLog) => {
      const editor = page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]');
      await editor.click();
      await page.keyboard.press("ControlOrMeta+A");
      await page.keyboard.type(text);
      expect((await editorShape(page)).text).toBe(text);
      if (flow === "on") {
        await expectNotDurablySaved(page);
        const started = performance.now();
        await persistBody(page);
        const id = await expectMatchingPersistAck(page, observed);
        return { id, latency: performance.now() - started };
      }
      await expect(page.locator('[data-body-mode="off"]')).toHaveAttribute(
        "data-body-persisted",
        "false",
      );
      const matches = (value: string) => new URL(value).pathname === `${path}/body/versioned`;
      const sent = page.waitForRequest((r) => r.method() === "PUT" && matches(r.url()));
      const received = page.waitForResponse(
        (r) => r.request().method() === "PUT" && matches(r.url()),
      );
      const started = performance.now();
      await page.getByRole("button", { name: "저장", exact: true }).click();
      const command = (await sent).postDataJSON() as Schema["SaveVersionedBodyInput"];
      const response = await received;
      expect(response.status()).toBe(200);
      const result = (await response.json()) as Schema["SaveVersionedBodyResponse"];
      expect(result.commandId).toBe(command.commandId);
      expect(result.targetId).toBe(document.id);
      expect(result.commandId).toMatch(UUID_RE);
      expect(result.revisionId).toMatch(UUID_RE);
      expect(BigInt(result.tailSeq)).toBeGreaterThan(BigInt(command.expectedTailSeq));
      await expect(page.locator('[data-body-mode="off"]')).toHaveAttribute(
        "data-body-persisted",
        "true",
      );
      return { id: result.commandId, latency: performance.now() - started };
    };
    await editSave(workload.warm, wire);
    await readback(workload.warm);
    await marker("warm");
    await marker("start");
    const ownerAtStart = ownerConnections();
    for (const [i, text] of workload.edits.entries()) {
      const saved = await editSave(text, wire);
      ackIds.push(saved.id);
      ackLatencyMs.push(saved.latency);
      const roomKeys = new Set(
        [...wire.sent, ...wire.received].flatMap((f) => ("routingKey" in f ? [f.routingKey] : [])),
      );
      const observedOwner = ownerConnections();
      resources.push({
        phase: `ack-${String(i)}`,
        ownerConnections: observedOwner,
        observedRoomKeys: roomKeys.size,
      });
      await info.attach(`owner-connections-${String(i)}.json`, {
        contentType: "application/json",
        body: JSON.stringify(observedOwner),
      });
      expect(observedOwner.collabActive).toBe(flow === "on" ? 1 : 0);
      expect(roomKeys.size).toBe(flow === "on" ? 1 : 0);
      await marker(`ack-${String(i)}`);
      await readback(text);
      await marker(`read-${String(i)}`);
    }
    await context.close();
    ownerClosed = true;
    const ownerAfterClose = ownerConnections();
    expect(ownerAfterClose.contextClosed).toBe(true);
    expect(ownerAfterClose.active).toBe(0);
    await marker("closed");
    expect(phase).toBe(phases.length);
    await info.attach("equal-work.json", {
      contentType: "application/json",
      body: JSON.stringify({
        backend,
        flow,
        workloadHash,
        warmSaves: 1,
        warmReadbacks: 1,
        edits: 3,
        acks: 3,
        freshReadbacks: 3,
        texts: workload.edits,
        ackIds,
        ackLatencyMs,
        resources,
        ownerAtStart,
        ownerAfterClose,
        freshConnections,
        contextsClosed: true,
        roomDefinition:
          "unique owner wire routing keys; collaboration sockets use the existing /collab classification and WebSocket.isClosed; totals are separate; context close is positive retirement evidence; not a server room registry gauge",
        latencyDefinition:
          "Playwright runner monotonic clock: before Save click through matching ACK and persisted UI confirmation",
      }),
    });
  } finally {
    if (!ownerClosed) await context.close();
  }
});
