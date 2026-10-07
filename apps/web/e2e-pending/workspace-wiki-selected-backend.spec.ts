/**
 * One identical current Vue flow for the root's isolated PostgreSQL/SQLite
 * normal-main runs. The root owns preparation, fresh dist/native binaries,
 * backend/role/FK witnesses, port-zero launch, resources and CI registration.
 * This imports base Playwright, not the PG-only collabApp server fixture.
 * The selected SQLite actor fixture opens only the root's existing isolated
 * DB after Vue setup; its explicit inputs and fresh binary are required.
 */
import { closeSync, constants, fstatSync, lstatSync, openSync, readFileSync } from "node:fs";
import { isAbsolute } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import * as Y from "yjs";
import { MessageType } from "@hocuspocus/provider";
import * as decoding from "lib0/decoding";
import * as encoding from "lib0/encoding";
import * as sync from "y-protocols/sync";
import { yDocToTiptapJson } from "../../../packages/editor/src/collab-tiptap";
import { emojiGlyph } from "../../../packages/editor/src/emoji-glyph";
import { extractText, walkTiptap, type TiptapWalkNode } from "../../../packages/editor/src/extract";
import type { components } from "../src/generated/api";
import { watchCspViolations } from "../e2e/helpers";
import {
  admin,
  attachCollabWire,
  closeCollabContext,
  editorShape,
  ensureInstanceSetup,
  expectAwarenessTokenNotSession,
  expectMatchingPersistAck,
  expectNotDurablySaved,
  expectTokens,
  login,
  member,
  newCollabContext,
  openEditor,
  persistBody,
  waitConnected,
} from "./collab-helpers";
import { decodeHocuspocusFrame, frameBytes, SESSION_COOKIE, UUID_RE } from "./collab-wire";
import { expectSelectedWikiAuxiliary } from "./workspace-wiki-selected-auxiliary";
import {
  requiredFixtureInput,
  installSelectedMember,
  selectedSetupNeeded,
} from "./selected-backend-fixture";

type DocumentMeta = components["schemas"]["DocumentMetaResponse"];
type RevisionDetail = components["schemas"]["RevisionDetailResponse"];
type BodyResponse = components["schemas"]["BodyResponse"];
type SessionUser = components["schemas"]["SessionUserOutput"];
type CreateBody = components["schemas"]["CreateDocumentBody"];

async function currentUser(page: Page): Promise<SessionUser> {
  const response = await page.request.get("/api/v1/auth/me");
  expect(response.status()).toBe(200);
  const user = (await response.json()) as SessionUser;
  expect(user.userId).toMatch(UUID_RE);
  expect(user.sessionId).toMatch(UUID_RE);
  return user;
}

async function body(page: Page, path: string): Promise<BodyResponse> {
  const response = await page.request.get(`${path}/body`);
  expect(response.status()).toBe(200);
  return (await response.json()) as BodyResponse;
}

async function revision(page: Page, path: string, id: string): Promise<RevisionDetail> {
  const response = await page.request.get(`${path}/revisions/${id}`);
  expect(response.status()).toBe(200);
  const detail = (await response.json()) as RevisionDetail;
  expect(detail.id).toBe(id);
  return detail;
}

function documentNodeIds(value: unknown): string[] {
  if (!value || typeof value !== "object" || Array.isArray(value)) return [];
  const node = value as { attrs?: { id?: unknown }; content?: unknown[] };
  const own = typeof node.attrs?.id === "string" ? [node.attrs.id] : [];
  return [...own, ...(node.content ?? []).flatMap(documentNodeIds)];
}

function expectCanonicalContent(
  content: unknown,
  retained: string,
  removed: string,
  expectedIds: string[],
): void {
  const text = extractText(content);
  expect(text).toBe(`${retained} 새 편집`);
  expect(text).not.toContain(removed);
  const emojis: TiptapWalkNode[] = [];
  walkTiptap(content, (node) => {
    if (node.type === "emoji") emojis.push(node);
  });
  expect(emojis).toHaveLength(1);
  expect(emojis[0].attrs?.name).toBe("grinning");
  expect(emojiGlyph(emojis[0])).toBe("😀");
  const ids = documentNodeIds(content);
  expect(ids.length).toBeGreaterThan(0);
  for (const id of ids) expect(id).toMatch(UUID_RE);
  expect(ids).toEqual(expectedIds);
}

function expectCanonicalOracleControls(
  content: unknown,
  retained: string,
  removed: string,
  ids: string[],
): string[] {
  // Mutate read-only JSON copies using the maintained walker. These controls
  // never touch a live editor/Y.Doc, native bytes, or server state.
  const controls: [string, (copy: unknown) => void][] = [
    [
      "emoji absent",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (node.type === "emoji") {
            node.type = "text";
            node.text = "";
            delete node.attrs;
          }
        });
      },
    ],
    [
      "wrong emoji name despite matching explicit glyph",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (node.type === "emoji")
            node.attrs = { ...node.attrs, name: "wrong-name", emoji: "😀" };
        });
      },
    ],
    [
      "emoji misplaced",
      (copy) => {
        let emoji: TiptapWalkNode | undefined;
        walkTiptap(copy, (node) => {
          if (node.type === "emoji") emoji = node;
        });
        walkTiptap(copy, (node) => {
          if (emoji && node.content?.includes(emoji))
            node.content = [emoji, ...node.content.filter((child) => child !== emoji)];
        });
      },
    ],
    [
      "retained text missing",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (typeof node.text === "string") node.text = node.text.replace("동일한", "");
        });
      },
    ],
    [
      "deleted text survives",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (typeof node.text === "string" && node.text.includes("새 편집")) node.text += removed;
        });
      },
    ],
    [
      "block identity changed",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (node.type === "paragraph")
            node.attrs = { ...node.attrs, id: "00000000-0000-4000-8000-000000000340" };
        });
      },
    ],
  ];
  expectCanonicalContent(content, retained, removed, ids);
  for (const [label, mutate] of controls) {
    const copy = structuredClone(content);
    mutate(copy);
    expect(() => {
      expectCanonicalContent(copy, retained, removed, ids);
    }, label).toThrow();
  }
  return controls.map(([label]) => label);
}

type NativeWireFrame = { direction: "sent" | "received"; room: string; bytes: Uint8Array };

function observeNativeWire(page: Page): NativeWireFrame[] {
  const frames: NativeWireFrame[] = [];
  const syncTypes: readonly number[] = [MessageType.Sync];
  page.on("websocket", (socket) => {
    if (!socket.url().includes("/collab")) return;
    const observe = (direction: "sent" | "received", payload: string | Buffer) => {
      const bytes = frameBytes(payload);
      const frame = decodeHocuspocusFrame(bytes);
      if (frame?.kind === "other" && syncTypes.includes(frame.type))
        frames.push({ direction, room: frame.routingKey, bytes: Uint8Array.from(bytes) });
    };
    socket.on("framesent", (frame) => {
      observe("sent", frame.payload);
    });
    socket.on("framereceived", (frame) => {
      observe("received", frame.payload);
    });
  });
  return frames;
}

function nativeHistory(frames: NativeWireFrame[], room: string, receivedOnly: boolean): Y.Doc {
  const native = new Y.Doc({ gc: false });
  try {
    let receivedStep2 = 0;
    for (const frame of frames) {
      if (frame.room !== room || (receivedOnly && frame.direction !== "received")) continue;
      // Reuse the existing Hocuspocus frame filter and installed public codecs.
      // The response encoder is a local sink: no reply is sent to any socket.
      const decoder = decoding.createDecoder(frame.bytes);
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
      expect(decoder.pos).toBe(frame.bytes.length);
      if (frame.direction === "received" && kind === sync.messageYjsSyncStep2) receivedStep2++;
    }
    expect(receivedStep2, "real server full-sync response is required").toBeGreaterThan(0);
    return native;
  } catch (error) {
    native.destroy();
    throw error;
  }
}

function expectRetainedNative(
  detail: RevisionDetail,
  native: Y.Doc,
  retained: string,
  removed: string,
): void {
  // Revision ySnapshot is a DSSV snapshot, not a full update. Its full retained
  // native history comes only from observed real sync frames (gc=false).
  const snapshot = Y.decodeSnapshot(Uint8Array.from(Buffer.from(detail.ySnapshot, "base64")));
  expect(snapshot.sv.size).toBeGreaterThan(0);
  expect(snapshot.ds.clients.size, "revision snapshot retains the deletion set").toBeGreaterThan(0);
  expect(
    Y.equalSnapshots(Y.snapshot(native), snapshot),
    "revision matches observed native state",
  ).toBe(true);
  const update = Y.encodeStateAsUpdate(native);
  expect(update.byteLength).toBeGreaterThan(2);
  const decoded = Y.decodeUpdate(update);
  expect(decoded.ds.clients.size, "full native history retains the deletion set").toBeGreaterThan(
    0,
  );
  const retainedStrings = decoded.structs.flatMap((struct) =>
    struct instanceof Y.Item && struct.content instanceof Y.ContentString
      ? [struct.content.str]
      : [],
  );
  expect(retainedStrings.join(""), "deleted source bytes survive the native history").toContain(
    removed,
  );
  const xml: unknown = native.getXmlFragment("prosemirror").toJSON();
  if (typeof xml !== "string") throw new Error("native XML serialization must be a string");
  expect([...native.share.keys()]).toEqual(["prosemirror"]);
  const ids = documentNodeIds(detail.contentJson);
  expectCanonicalContent(yDocToTiptapJson(native), retained, removed, ids);
  expect(xml).not.toContain(removed);
  for (const id of ids) expect(xml).toContain(id);
  const historical = new Y.Doc({ gc: false });
  try {
    Y.createDocFromSnapshot(native, snapshot, historical);
    const content = yDocToTiptapJson(historical);
    expect(content).toEqual(detail.contentJson);
    expectCanonicalContent(content, retained, removed, ids);
  } finally {
    historical.destroy();
  }
}

function expectNativeHistoryControls(
  detail: RevisionDetail,
  native: Y.Doc,
  retained: string,
  removed: string,
): string[] {
  expectRetainedNative(detail, native, retained, removed);
  const snapshot = Y.decodeSnapshot(Buffer.from(detail.ySnapshot, "base64"));
  const wrongVector = new Map(snapshot.sv);
  const first = wrongVector.entries().next().value;
  if (!first) throw new Error("nonempty native snapshot state vector is required");
  wrongVector.set(first[0], first[1] + 1);
  const wrongSnapshot = Y.encodeSnapshot(Y.createSnapshot(snapshot.ds, wrongVector));
  expect(() => {
    expectRetainedNative(
      { ...detail, ySnapshot: Buffer.from(wrongSnapshot).toString("base64") },
      native,
      retained,
      removed,
    );
  }, "a valid but wrong revision snapshot must fail").toThrow();
  const missingHistory = new Y.Doc({ gc: true });
  try {
    // A native-byte observer copy only, never a live editor or JSON reseed.
    Y.applyUpdate(missingHistory, Y.encodeStateAsUpdate(native));
    missingHistory.gc = false;
    expect(Y.equalSnapshots(Y.snapshot(missingHistory), snapshot)).toBe(true);
    expect(yDocToTiptapJson(missingHistory)).toEqual(detail.contentJson);
    expect(() => {
      expectRetainedNative(detail, missingHistory, retained, removed);
    }, "matching text/snapshot without deleted native source bytes must fail").toThrow();
  } finally {
    missingHistory.destroy();
  }
  return ["valid wrong snapshot", "same text and snapshot but missing deleted native source bytes"];
}

test("selected normal main: Vue setup, stable wiki create, native persist, manual revision and fresh actor readback", async ({
  browser,
  baseURL,
}, testInfo) => {
  const selected = process.env.FVOCI_E2E_SELECTED_BACKEND;
  expect(selected, "root must identify the actual isolated selected-backend run").toMatch(
    /^(postgres|sqlite|libsql-remote)$/,
  );
  if (!baseURL) throw new Error("root-provided selected normal-main baseURL is required");
  testInfo.annotations.push({ type: "selected-backend", description: selected ?? "missing" });
  const ctxA = await newCollabContext(browser, baseURL);
  const ctxB = await newCollabContext(browser, baseURL);
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  const wireA = attachCollabWire(pageA);
  const wireB = attachCollabWire(pageB);
  const nativeWireA = observeNativeWire(pageA);
  const nativeWireB = observeNativeWire(pageB);
  const cspA = watchCspViolations(pageA);
  const cspB = watchCspViolations(pageB);
  let failed = true;
  try {
    const initialSetup = await pageA.request.get("/api/v1/setup");
    expect(initialSetup.status()).toBe(200);
    expect(await initialSetup.json()).toMatchObject({ needed: selectedSetupNeeded() });
    if (selectedSetupNeeded()) await ensureInstanceSetup(pageA);
    await expect(pageA.locator("#root[data-v-app]")).toHaveCount(1);
    await login(pageA, admin.email, admin.password);
    const creator = await currentUser(pageA);
    const cookieA = (await ctxA.cookies()).find((cookie) => cookie.name === SESSION_COOKIE);
    expect(cookieA?.httpOnly).toBe(true);
    expect(cookieA?.sameSite).toBe("Lax");
    const workspaces = await pageA.request.get("/api/v1/me/workspaces");
    expect(workspaces.status()).toBe(200);
    const workspace = (
      (await workspaces.json()) as components["schemas"]["WorkspaceListResponse"]
    ).items.find((item) => item.slug === admin.workspaceSlug);
    if (!workspace) throw new Error("real setup workspace missing");
    expect(workspace.role).toBe("owner");
    const documents = `/api/v1/workspaces/${workspace.id}/documents`;
    await pageA.goto(`/w/${admin.workspaceSlug}/wiki`);
    await expect(pageA.locator("#root[data-v-app]")).toHaveCount(1);
    const sent = pageA.waitForRequest(
      (request) => request.method() === "POST" && new URL(request.url()).pathname === documents,
    );
    const created = pageA.waitForResponse(
      (response) =>
        response.request().method() === "POST" && new URL(response.url()).pathname === documents,
    );
    await pageA.getByRole("button", { name: "새 문서", exact: true }).click();
    const command = (await sent).postDataJSON() as CreateBody;
    expect(command.commandId).toMatch(UUID_RE);
    expect(command.parentId).toBeNull();
    expect(command.title).toBe("제목 없음");
    const createResponse = await created;
    expect(createResponse.status()).toBe(201);
    const meta = (await createResponse.json()) as DocumentMeta;
    expect(meta.id).toMatch(UUID_RE);
    expect(meta.createdBy).toBe(creator.userId);
    expect(meta.workspaceId).toBe(workspace.id);
    expect(meta.projectId).toBeNull();
    const displayId = `WIKI-${String(meta.number)}`;
    expect(meta.displayId).toBe(displayId);
    // Replays use exactly the command emitted by the current WikiPage.
    for (let retry = 0; retry < 2; retry++) {
      const replay = await pageA.request.post(documents, { data: command });
      expect(replay.status()).toBe(201);
      expect(await replay.json()).toEqual(meta);
    }
    const mismatch = await pageA.request.post(documents, {
      data: { ...command, title: "changed hash" },
    });
    expect(mismatch.status()).toBe(409);
    const documentPath = `${documents}/${meta.id}`;
    const editorA = await openEditor(pageA, `/w/${admin.workspaceSlug}/${displayId}`);
    const retained = "동일한 저장 문장 한글😀";
    const removed = "delete me";
    await editorA.click();
    await pageA.keyboard.type(`${retained} ${removed}`);
    await expectNotDurablySaved(pageA);
    await persistBody(pageA);
    const firstAck = await expectMatchingPersistAck(pageA, wireA);
    // A deletion and a later edit cannot be made clean by the old matching ACK.
    await editorA.focus();
    await pageA.keyboard.press("ControlOrMeta+End");
    for (let index = 0; index < removed.length; index++) await pageA.keyboard.press("Backspace");
    await pageA.keyboard.type("새 편집");
    await expectNotDurablySaved(pageA);
    await persistBody(pageA);
    const finalAck = await expectMatchingPersistAck(pageA, wireA);
    expect(finalAck).not.toBe(firstAck);
    const persisted = await body(pageA, documentPath);
    expect(persisted.version).toBeGreaterThan(0);
    const beforeRevision = await editorShape(pageA);
    const oracleControls = expectCanonicalOracleControls(
      persisted.contentJson,
      retained,
      removed,
      documentNodeIds(beforeRevision.document),
    );
    const savedRevision = pageA.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname === `${documentPath}/revisions`,
    );
    await pageA.getByTestId("revision-history").click();
    await pageA.getByTestId("revision-save").click();
    const revisionResponse = await savedRevision;
    expect(revisionResponse.status()).toBe(201);
    const revisionId = (
      (await revisionResponse.json()) as components["schemas"]["RevisionCreateResponse"]
    ).id;
    expect(revisionId).toMatch(UUID_RE);
    const saved = await revision(pageA, documentPath, revisionId);
    expect(saved).toMatchObject({
      targetId: meta.id,
      targetKind: "document",
      reason: "manual",
      createdBy: creator.userId,
      restoredFromId: null,
    });
    expect(saved.contentJson).toEqual(persisted.contentJson);
    const room = `${workspace.id}:document:${meta.id}`;
    const nativeA = nativeHistory(nativeWireA, room, false);
    let nativeControls: string[];
    try {
      nativeControls = expectNativeHistoryControls(saved, nativeA, retained, removed);
    } finally {
      nativeA.destroy();
    }

    const fixtureUser = installSelectedMember(selected ?? "missing");
    await login(pageB, member.email, member.password);
    const reader = await currentUser(pageB);
    expect(reader.userId).not.toBe(creator.userId);
    if (fixtureUser) expect(reader.userId).toBe(fixtureUser);
    expect(reader.sessionId).not.toBe(creator.sessionId);
    const cookieB = (await ctxB.cookies()).find((cookie) => cookie.name === SESSION_COOKIE);
    expect(cookieB?.httpOnly).toBe(true);
    expect(cookieB?.value).not.toBe(cookieA?.value);
    const wrongActor = await pageB.request.post(documents, { data: command });
    expect(wrongActor.status()).toBe(409);
    await openEditor(pageB, `/w/${admin.workspaceSlug}/${displayId}`);
    await expect(pageB.locator("#root[data-v-app]")).toHaveCount(1);
    await expectTokens(pageB, [retained, "새 편집"]);
    await expectAwarenessTokenNotSession(wireB, cookieB?.value ?? "missing");
    await expect(pageB.getByRole("button", { name: "저장", exact: true })).toBeEnabled();
    expect(await editorShape(pageB)).toEqual(beforeRevision);
    expect(await body(pageB, documentPath)).toEqual(persisted);
    const readMeta = await pageB.request.get(documentPath);
    expect(readMeta.status()).toBe(200);
    expect(await readMeta.json()).toMatchObject({
      id: meta.id,
      number: meta.number,
      workspaceId: workspace.id,
      path: meta.path,
      parentId: null,
      projectId: null,
      createdBy: creator.userId,
    });
    const freshRevision = await revision(pageB, documentPath, revisionId);
    expect(freshRevision).toEqual(saved);
    // The fresh actor's server-received full sync must independently contain
    // retained history; the creator's local edit cache cannot satisfy this.
    const nativeB = nativeHistory(nativeWireB, room, true);
    try {
      expectRetainedNative(freshRevision, nativeB, retained, removed);
    } finally {
      nativeB.destroy();
    }
    // A new page/socket in the independent context reads durable state again.
    const freshConnectionStart = nativeWireB.length;
    await pageB.reload();
    await waitConnected(pageB);
    await expectTokens(pageB, [retained, "새 편집"]);
    expect(await editorShape(pageB)).toEqual(beforeRevision);
    const reloadedRevision = await revision(pageB, documentPath, revisionId);
    expect(reloadedRevision).toEqual(saved);
    const reloadedNative = nativeHistory(nativeWireB.slice(freshConnectionStart), room, true);
    try {
      expectRetainedNative(reloadedRevision, reloadedNative, retained, removed);
    } finally {
      reloadedNative.destroy();
    }
    expect(cspA).toEqual([]);
    expect(cspB).toEqual([]);
    await testInfo.attach("selected-vue-native-readback.json", {
      contentType: "application/json",
      body: Buffer.from(
        JSON.stringify(
          {
            selected,
            workspaceId: workspace.id,
            document: meta,
            command,
            firstAck,
            finalAck,
            persisted,
            revision: saved,
            canonicalEmojiOracleControls: oracleControls,
            nativeHistoryOracleControls: nativeControls,
            nativeHistoryReadback: {
              room,
              creator: "observed sent and received native sync",
              freshActor: "server-received native sync only",
              reloadedActor: "new connection server-received native sync only",
            },
            creatorId: creator.userId,
            freshActorId: reader.userId,
          },
          null,
          2,
        ),
      ),
    });
    if (process.env.FVOCI_E2E_SELECTED_AUXILIARY !== undefined) {
      expect(process.env.FVOCI_E2E_SELECTED_AUXILIARY).toBe("normal-api");
      await expectSelectedWikiAuxiliary({
        browser,
        baseURL,
        ownerPage: pageA,
        selected: selected ?? "missing",
        workspaceId: workspace.id,
        document: meta,
        creatorId: creator.userId,
        sourceBlockId: documentNodeIds(persisted.contentJson)[0],
        reader,
        persisted,
        revision: saved,
        testInfo,
      });
    }
    failed = false;
  } finally {
    await Promise.all([closeCollabContext(ctxA, failed), closeCollabContext(ctxB, failed)]);
  }
});

type RestartCheckpoint = {
  schema: 1;
  source: string;
  tree: string;
  compiledSource: string;
  selected: "postgres" | "sqlite" | "libsql-remote";
  stopped: { serverExit: 0; portClosed: true; recordedIdentitiesRetired: true };
  seed: {
    selected: string;
    workspaceId: string;
    document: DocumentMeta;
    command: CreateBody;
    firstAck: string;
    finalAck: string;
    persisted: BodyResponse;
    revision: RevisionDetail;
    creatorId: string;
    freshActorId: string;
    canonicalEmojiOracleControls: string[];
    nativeHistoryOracleControls: string[];
  };
};

function expectRestartCheckpoint(
  checkpoint: RestartCheckpoint,
  selected: string,
  source: string,
): void {
  expect(source).toMatch(/^[0-9a-f]{40}$/);
  expect(checkpoint.schema).toBe(1);
  expect(checkpoint.source).toBe(source);
  expect(checkpoint.compiledSource).toBe(source);
  expect(checkpoint.tree).toMatch(/^[0-9a-f]{40}$/);
  expect(checkpoint.selected).toBe(selected);
  expect(checkpoint.stopped).toEqual({
    serverExit: 0,
    portClosed: true,
    recordedIdentitiesRetired: true,
  });
  const seed = checkpoint.seed;
  expect(seed.selected).toBe(selected);
  for (const id of [
    seed.workspaceId,
    seed.document.id,
    seed.command.commandId,
    seed.revision.id,
    seed.creatorId,
    seed.freshActorId,
    seed.firstAck,
    seed.finalAck,
  ])
    expect(id).toMatch(UUID_RE);
  expect(seed.firstAck).not.toBe(seed.finalAck);
  expect(seed.freshActorId).not.toBe(seed.creatorId);
  expect(seed.document.workspaceId).toBe(seed.workspaceId);
  expect(seed.document.displayId).toBe(`WIKI-${String(seed.document.number)}`);
  expect(seed.revision.targetId).toBe(seed.document.id);
  expect(seed.revision.createdBy).toBe(seed.creatorId);
  expect(seed.revision.reason).toBe("manual");
  expect(seed.revision.contentJson).toEqual(seed.persisted.contentJson);
  expect(seed.canonicalEmojiOracleControls).toHaveLength(6);
  expect(seed.nativeHistoryOracleControls).toHaveLength(2);
}

function readRestartCheckpoint(path: string): RestartCheckpoint {
  if (!isAbsolute(path)) throw new Error("restart checkpoint must be an explicit absolute file");
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const metadata = fstatSync(fd);
    const named = lstatSync(path);
    if (
      !metadata.isFile() ||
      metadata.nlink !== 1 ||
      metadata.uid !== process.getuid?.() ||
      (metadata.mode & 0o777) !== 0o600 ||
      metadata.size > 128 * 1024 ||
      named.dev !== metadata.dev ||
      named.ino !== metadata.ino
    )
      throw new Error("restart checkpoint must be a bounded owned private regular file");
    return JSON.parse(readFileSync(fd, "utf8")) as RestartCheckpoint;
  } finally {
    closeSync(fd);
  }
}

// An explicit root checkpoint registers the second observation. Ordinary
// selected runs still discover exactly the accepted original case above.
// The root runs this title alone in a NEW browser process after server exit;
// no seed helper, setup, editor write or native cache is reused here.
if (process.env.FVOCI_E2E_SELECTED_RESTART_CHECKPOINT !== undefined) {
  test("selected normal main restart: fresh actor reads persisted native history and manual revision", async ({
    browser,
    baseURL,
  }, testInfo) => {
    const selected = requiredFixtureInput("FVOCI_E2E_SELECTED_BACKEND");
    expect(selected).toMatch(/^(postgres|sqlite|libsql-remote)$/);
    const source = requiredFixtureInput("FVOCI_E2E_SELECTED_RESTART_SOURCE");
    const checkpoint = readRestartCheckpoint(
      requiredFixtureInput("FVOCI_E2E_SELECTED_RESTART_CHECKPOINT"),
    );
    expectRestartCheckpoint(checkpoint, selected, source);
    if (!baseURL) throw new Error("root-provided restarted normal-main baseURL is required");
    const seed = checkpoint.seed;
    const ctx = await newCollabContext(browser, baseURL);
    const page = await ctx.newPage();
    const wire = attachCollabWire(page);
    const nativeWire = observeNativeWire(page);
    const csp = watchCspViolations(page);
    let failed = true;
    try {
      const setup = await page.request.get("/api/v1/setup");
      expect(setup.status()).toBe(200);
      expect(await setup.json()).toMatchObject({ needed: false });
      await login(page, member.email, member.password);
      const reader = await currentUser(page);
      expect(reader.userId).toBe(seed.freshActorId);
      expect(reader.userId).not.toBe(seed.creatorId);
      const cookie = (await ctx.cookies()).find((entry) => entry.name === SESSION_COOKIE);
      expect(cookie?.httpOnly).toBe(true);
      expect(cookie?.sameSite).toBe("Lax");
      const workspaces = await page.request.get("/api/v1/me/workspaces");
      expect(workspaces.status()).toBe(200);
      const workspace = (
        (await workspaces.json()) as components["schemas"]["WorkspaceListResponse"]
      ).items.find((item) => item.id === seed.workspaceId);
      expect(workspace).toMatchObject({ slug: admin.workspaceSlug, role: "member" });
      const documentPath = `/api/v1/workspaces/${seed.workspaceId}/documents/${seed.document.id}`;
      expect(Number.isSafeInteger(seed.document.number)).toBe(true);
      expect(seed.document.number).toBeGreaterThan(0);
      const displayId = `WIKI-${String(seed.document.number)}`;
      await openEditor(page, `/w/${admin.workspaceSlug}/${displayId}`);
      await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
      const retained = "동일한 저장 문장 한글😀";
      const removed = "delete me";
      await expectTokens(page, [retained, "새 편집"]);
      await expectAwarenessTokenNotSession(wire, cookie?.value ?? "missing");
      await expect(page.getByRole("button", { name: "저장", exact: true })).toBeEnabled();
      const currentBody = await body(page, documentPath);
      expect(currentBody).toEqual(seed.persisted);
      const shape = await editorShape(page);
      expectCanonicalContent(
        shape.document,
        retained,
        removed,
        documentNodeIds(seed.persisted.contentJson),
      );
      // Wiki URLs resolve the persisted number in the visible workspace tree,
      // as useWikiDocumentRef does; generic document metadata has no displayId.
      const treeResponse = await page.request.get(`/api/v1/workspaces/${seed.workspaceId}/tree`);
      expect(treeResponse.status()).toBe(200);
      const tree = (await treeResponse.json()) as components["schemas"]["TreeResponse"];
      const resolved = tree.items.filter(
        (item) => item.projectId === null && item.number === seed.document.number,
      );
      expect(resolved).toHaveLength(1);
      expect(resolved[0]).toMatchObject({
        id: seed.document.id,
        workspaceId: seed.workspaceId,
        number: seed.document.number,
        title: seed.document.title,
        path: seed.document.path,
        parentId: null,
        projectId: null,
      });
      expect(displayId).toBe(seed.document.displayId);
      await expect(page).toHaveURL(new URL(`/w/${admin.workspaceSlug}/${displayId}`, baseURL).href);
      const currentMeta = await page.request.get(documentPath);
      expect(currentMeta.status()).toBe(200);
      expect(await currentMeta.json()).toMatchObject({
        id: seed.document.id,
        workspaceId: seed.workspaceId,
        number: seed.document.number,
        title: seed.document.title,
        path: seed.document.path,
        parentId: null,
        projectId: null,
        createdBy: seed.creatorId,
      });
      const currentRevision = await revision(page, documentPath, seed.revision.id);
      expect(currentRevision).toEqual(seed.revision);
      const room = `${seed.workspaceId}:document:${seed.document.id}`;
      const native = nativeHistory(nativeWire, room, true);
      let nativeControls: string[];
      try {
        nativeControls = expectNativeHistoryControls(currentRevision, native, retained, removed);
      } finally {
        native.destroy();
      }
      const oracleControls = expectCanonicalOracleControls(
        currentBody.contentJson,
        retained,
        removed,
        documentNodeIds(seed.persisted.contentJson),
      );
      const freshConnectionStart = nativeWire.length;
      await page.reload();
      await waitConnected(page);
      await expectTokens(page, [retained, "새 편집"]);
      expect(await editorShape(page)).toEqual(shape);
      expect(await body(page, documentPath)).toEqual(seed.persisted);
      const reloadedRevision = await revision(page, documentPath, seed.revision.id);
      expect(reloadedRevision).toEqual(seed.revision);
      const reloadedNative = nativeHistory(nativeWire.slice(freshConnectionStart), room, true);
      try {
        expectRetainedNative(reloadedRevision, reloadedNative, retained, removed);
      } finally {
        reloadedNative.destroy();
      }
      expect(csp).toEqual([]);
      await testInfo.attach("selected-vue-restart-readback.json", {
        contentType: "application/json",
        body: Buffer.from(
          JSON.stringify({
            selected,
            source,
            tree: checkpoint.tree,
            workspaceId: seed.workspaceId,
            documentId: seed.document.id,
            freshActorId: reader.userId,
            sessionId: reader.sessionId,
            persisted: currentBody,
            revision: currentRevision,
            firstAck: seed.firstAck,
            finalAck: seed.finalAck,
            ackPhase: "two pre-restart seed ACKs; readback performs no new edit",
            canonicalEmojiOracleControls: oracleControls,
            nativeHistoryOracleControls: nativeControls,
            nativeHistoryReadback:
              "fresh browser and reloaded connection server-received sync only",
          }),
        ),
      });
      failed = false;
    } finally {
      await closeCollabContext(ctx, failed);
    }
  });
}
