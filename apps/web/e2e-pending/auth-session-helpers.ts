/** Narrow browser operations for the allocated normal auth acceptance server. */
import { expect, type Browser, type BrowserContext, type Page, type Route } from "@playwright/test";
import * as Y from "yjs";
import { MessageType } from "@hocuspocus/provider";
import * as decoding from "lib0/decoding";
import * as encoding from "lib0/encoding";
import * as sync from "y-protocols/sync";
import { yDocToTiptapJson } from "../../../packages/editor/src/collab-tiptap";
import { walkTiptap } from "../../../packages/editor/src/extract";
import type { components } from "../src/generated/api";
import { newCollabContext, openEditor } from "./collab-helpers";
import { decodeHocuspocusFrame, frameBytes, UUID_RE } from "./collab-wire";
import type { AuthNoEffect } from "./auth-session-contract";

type Schema = components["schemas"];
export type AuthTarget = { id: string; workspaceId: string; path: string; url: string };
export type AuthActor = { context: BrowserContext; page: Page; user: Schema["SessionUserOutput"] };
export type AuthFlow = "on" | "off";

export async function currentSession(page: Page): Promise<Schema["SessionUserOutput"]> {
  const response = await page.request.get("/api/v1/auth/me");
  expect(response.status()).toBe(200);
  const user = (await response.json()) as Schema["SessionUserOutput"];
  expect(user.userId).toMatch(UUID_RE);
  expect(user.sessionId).toMatch(UUID_RE);
  return user;
}

/** Read every page; a truncated list cannot prove no effects. */
async function fullList(page: Page, path: string): Promise<Record<string, unknown>[]> {
  const rows: Record<string, unknown>[] = [];
  const cursors = new Set<string>();
  let cursor: string | null = null;
  for (let count = 0; count < 100; count++) {
    const response = await page.request.get(path, {
      params: { limit: "100", ...(cursor ? { cursor } : {}) },
    });
    expect(response.status()).toBe(200);
    const data = (await response.json()) as {
      items: Record<string, unknown>[];
      nextCursor: string | null;
    };
    expect(Array.isArray(data.items)).toBe(true);
    rows.push(...data.items);
    if (data.nextCursor === null) return rows;
    expect(typeof data.nextCursor).toBe("string");
    expect(cursors.has(data.nextCursor)).toBe(false);
    cursors.add(data.nextCursor);
    cursor = data.nextCursor;
  }
  throw new Error("Auth no-effect observation exceeded the complete-list bound");
}

async function freshNative(page: Page, target: AuthTarget, flow: AuthFlow): Promise<string> {
  if (flow === "off") {
    const response = await page.request.get(`${target.path}/body/versioned`);
    expect(response.status()).toBe(200);
    const body = (await response.json()) as Schema["VersionedBodyResponse"];
    expect(body.targetId).toBe(target.id);
    const native = new Y.Doc({ gc: false });
    try {
      Y.applyUpdate(native, Buffer.from(body.snapshotV1, "base64"));
      for (const tail of body.tailV1) Y.applyUpdate(native, Buffer.from(tail, "base64"));
      expect(yDocToTiptapJson(native)).toEqual(body.contentJson);
      return Buffer.from(Y.encodeStateAsUpdate(native)).toString("base64");
    } finally {
      native.destroy();
    }
  }
  const native = new Y.Doc({ gc: false });
  const room = `${target.workspaceId}:document:${target.id}`;
  let step2 = 0;
  page.on("websocket", (socket) => {
    if (!socket.url().endsWith("/collab")) return;
    socket.on("framereceived", ({ payload }) => {
      const bytes = frameBytes(payload);
      const frame = decodeHocuspocusFrame(bytes);
      const syncTypes: readonly number[] = [MessageType.Sync];
      if (frame?.kind !== "other" || !syncTypes.includes(frame.type) || frame.routingKey !== room)
        return;
      const decoder = decoding.createDecoder(bytes);
      expect(decoding.readVarString(decoder)).toBe(room);
      expect(decoding.readVarUint(decoder)).toBe(MessageType.Sync);
      const kind = sync.readSyncMessage(
        decoder,
        encoding.createEncoder(),
        native,
        null,
        (error) => {
          throw error;
        },
      );
      expect(decoder.pos).toBe(bytes.length);
      if (kind === sync.messageYjsSyncStep2) step2++;
    });
  });
  try {
    await openEditor(page, target.url);
    await expect.poll(() => step2).toBeGreaterThan(0);
    const response = await page.request.get(`${target.path}/body`);
    expect(response.status()).toBe(200);
    const body = (await response.json()) as Schema["BodyResponse"];
    expect(yDocToTiptapJson(native)).toEqual(body.contentJson);
    return Buffer.from(Y.encodeStateAsUpdate(native)).toString("base64");
  } finally {
    native.destroy();
  }
}

/** Fresh client, same proven healthy owner session; never seeds or repairs data. */
export async function documentObservation(
  browser: Browser,
  baseURL: string,
  owner: AuthActor,
  target: AuthTarget,
  flow: AuthFlow,
): Promise<AuthNoEffect> {
  const context = await newCollabContext(browser, baseURL);
  try {
    await context.addCookies(await owner.context.cookies());
    const page = await context.newPage();
    const user = await currentSession(page);
    expect(user.userId).toBe(owner.user.userId);
    expect(user.sessionId).toBe(owner.user.sessionId);
    const nativeV1 = await freshNative(page, target, flow);
    const response = await page.request.get(`${target.path}/body`);
    expect(response.status()).toBe(200);
    const body = (await response.json()) as Schema["BodyResponse"];
    const ids: string[] = [];
    walkTiptap(body.contentJson, (node) => {
      if (typeof node.attrs?.id === "string") ids.push(node.attrs.id);
    });
    const metadata = await fullList(page, `${target.path}/revisions`);
    const revisions = [];
    for (const item of metadata) {
      expect(item.id).toMatch(UUID_RE);
      const detail = await page.request.get(`${target.path}/revisions/${String(item.id)}`);
      expect(detail.status()).toBe(200);
      revisions.push((await detail.json()) as Schema["RevisionDetailResponse"]);
    }
    const events = (await fullList(page, `/api/v1/workspaces/${target.workspaceId}/events`)).filter(
      (event) => event.targetId === target.id,
    );
    revisions.sort((a, b) => a.id.localeCompare(b.id));
    events.sort((a, b) => String(a.id).localeCompare(String(b.id)));
    return { targetId: target.id, body, ids, nativeV1, revisions, events };
  } finally {
    await context.close();
  }
}

export async function activeRoom(
  page: Page,
): Promise<{ connections: number; closedCodes: number[] }> {
  const actual = { connections: 0, closedCodes: [] as number[] };
  await page.routeWebSocket("**/collab", (socket) => {
    const server = socket.connectToServer();
    actual.connections++;
    server.onClose(async (code, reason) => {
      if (code === undefined) throw new Error("Server close code is missing");
      actual.closedCodes.push(code);
      await socket.close({ code, reason });
    });
  });
  return actual;
}

/** Hold exactly the UI request until an explicit commit response releases it. */
export async function holdSave(page: Page, target: AuthTarget, flow: AuthFlow) {
  const method = flow === "off" ? "PUT" : "POST";
  const path = target.path + (flow === "off" ? "/body/versioned" : "/revisions");
  let held: Route | undefined;
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const handler = async (route: Route) => {
    if (held || route.request().method() !== method) return route.fallback();
    held = route;
    await gate;
    await route.continue();
  };
  await page.route(`**${path}`, handler);
  const response = page.waitForResponse(
    (r) => r.request().method() === method && new URL(r.url()).pathname === path,
  );
  if (flow === "off") {
    const editor = page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]');
    await editor.fill("must not commit after real session revocation");
    await expect(page.locator('[data-body-mode="off"]')).toHaveAttribute(
      "data-body-persisted",
      "false",
    );
    await page.getByRole("button", { name: "저장", exact: true }).click();
  } else {
    await page.getByTestId("revision-history").click();
    await page.getByTestId("revision-save").click();
  }
  await expect.poll(() => held !== undefined).toBe(true);
  if (!held) throw new Error("The actual pending save request was not observed");
  const command: unknown = held.request().postDataBuffer();
  if (flow === "off") {
    const input = held.request().postDataJSON() as Schema["SaveVersionedBodyInput"];
    expect(input.commandId).toMatch(UUID_RE);
    expect(input.updateV1.length).toBeGreaterThan(0);
  }
  return {
    release,
    async refused() {
      const actual = await response;
      expect(actual.status()).toBe(401);
      const direct = await page.request.fetch(path, {
        method,
        ...(command instanceof Buffer
          ? { data: command, headers: { "Content-Type": "application/json" } }
          : {}),
      });
      expect(direct.status()).toBe(401);
      await page.unroute(`**${path}`, handler);
      return {
        boundary: "pre-auth-http" as const,
        method,
        status: actual.status(),
        directStatus: direct.status(),
      };
    },
  };
}

export async function rejectedCookie(
  browser: Browser,
  baseURL: string,
  cookies: Awaited<ReturnType<BrowserContext["cookies"]>>,
  target: AuthTarget,
  flow: AuthFlow,
): Promise<void> {
  const context = await newCollabContext(browser, baseURL);
  try {
    await context.addCookies(cookies);
    const page = await context.newPage();
    expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
    expect((await page.request.get(`${target.path}/body`)).status()).toBe(401);
    expect((await page.request.post(`${target.path}/revisions`)).status()).toBe(401);
    const upgrade = await page.request.get("/collab", {
      headers: {
        Upgrade: "websocket",
        Connection: "Upgrade",
        "Sec-WebSocket-Version": "13",
        "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
        Origin: baseURL,
      },
    });
    // OFF disables the transport before authentication; 503 is not auth evidence.
    expect(upgrade.status()).toBe(flow === "on" ? 401 : 503);
  } finally {
    await context.close();
  }
}
