// Base Playwright consumes the caller's allocated normal server. No server or
// actor fixture starts here; all mutations below use real product UI/API.
import { randomUUID } from "node:crypto";
import { createHash } from "node:crypto";
import { walkTiptap } from "../../../packages/editor/src/extract";
import { expect, test, type WebSocketRoute } from "@playwright/test";
import type { components } from "../src/generated/api";
import { expectDocumentBoundaries } from "../e2e/document-boundary-oracle";
import { expectBlocks } from "../e2e/workspace-wiki-vue-editor";
import {
  attachmentNodes,
  editorShape,
  expectConverged,
  expectNotDurablySaved,
  expectTokens,
  insertSlashAttachment,
  insertSlashTable,
  storedAttachmentDownloadBytes,
  uniqueBlockIds,
  waitConnected,
} from "./collab-helpers";
import { UUID_RE } from "./collab-wire";
import {
  PRODUCT_TITLES,
  boundaryTitle,
  readFinalBinding,
  assertFinalMetadata,
} from "./selected-final-contract";
import {
  SelectedActors,
  body,
  freshReadback,
  mount,
  saveOff,
  saveOn,
  wiki,
  type Target,
} from "./selected-final-helpers";

type Schema = components["schemas"];
const { binding, sha256 } = readFinalBinding();
let actors: SelectedActors;

test.beforeAll(async ({ browser, baseURL }, info) => {
  assertFinalMetadata(info.config.metadata, sha256, binding);
  expect(readFinalBinding().sha256).toBe(sha256);
  expect(browser.browserType().name()).toBe(binding.browser);
  if (!baseURL) throw new Error("SELECTED_FINAL_ALLOCATION_REQUIRED");
  actors = new SelectedActors(browser, baseURL, binding);
  try {
    await actors.verifyAllocation();
  } catch (error) {
    await actors.close();
    throw error;
  }
});
test.beforeEach(({ browserName }, info) => {
  expect(browserName).toBe(binding.browser);
  expect(info.config.workers).toBe(1);
  assertFinalMetadata(info.config.metadata, sha256, binding);
  expect(readFinalBinding().sha256).toBe(sha256);
});
test.afterEach(async ({ browser }, info) => {
  expect(browser).toBe(actors.browser);
  const evidence = await actors.close();
  await info.attach("selected-final-writes.json", {
    contentType: "application/json",
    body: JSON.stringify({
      schema: 1,
      source: binding.source,
      tree: binding.tree,
      backend: binding.backend,
      flow: binding.flow,
      browser: binding.browser,
      ...evidence,
    }),
  });
});

if (binding.suite === "products") {
  test(PRODUCT_TITLES[0], async ({ browser }, info) => {
    expect(browser).toBe(actors.browser);
    const owner = await actors.open("owner"),
      other = await actors.open("member");
    await owner.page.goto(`/w/${binding.workspaceSlug}/wiki`);
    await owner.page.getByRole("button", { name: "개인 입력", exact: true }).click();
    const dialog = owner.page.getByRole("dialog", { name: "개인 입력", exact: true });
    await dialog.getByRole("radio", { name: "메모", exact: true }).check();
    await dialog
      .getByRole("textbox", { name: "제목 또는 짧은 기록", exact: true })
      .fill("private actual ON note");
    const sent = owner.page.waitForRequest(
      (r) => r.method() === "POST" && new URL(r.url()).pathname.endsWith("/personal-input"),
    );
    const received = owner.page.waitForResponse(
      (r) =>
        r.request().method() === "POST" && new URL(r.url()).pathname.endsWith("/personal-input"),
    );
    await dialog.getByRole("button", { name: "개인 공간에 저장", exact: true }).click();
    const request = await sent;
    const command = request.postDataJSON() as Schema["PersonalInputBody"];
    expect(command.requestId).toMatch(UUID_RE);
    expect(command.intent).toBe("note");
    const response = await received;
    expect(response.status()).toBe(201);
    const created = (await response.json()) as Schema["PersonalInputOutput"];
    expect(created.documentId).toMatch(UUID_RE);
    expect(created.taskId).toBeNull();
    expect(created.projectId).toBeNull();
    const replay = await owner.page.request.post(request.url(), { data: command });
    expect(replay.status()).toBe(201);
    expect(await replay.json()).toEqual({ ...created, replayed: true });
    await dialog.getByRole("button", { name: "메모 열기", exact: true }).click();
    await waitConnected(owner.page);
    const workspaceId = new URL(request.url()).pathname.split("/")[4];
    expect(workspaceId).toMatch(UUID_RE);
    if (!workspaceId) throw new Error("SELECTED_FINAL_PERSONAL_TARGET_MISSING");
    const target: Target = {
      id: created.documentId,
      workspaceId,
      path: `/api/v1/workspaces/${workspaceId}/documents/${created.documentId}`,
      url: new URL(owner.page.url()).pathname,
    };
    await owner.page.locator(".fvoci-editor .ProseMirror").click();
    await owner.page.keyboard.type("개인 기록 한글😀");
    await expectNotDurablySaved(owner.page);
    await saveOn(owner);
    const expected = await editorShape(owner.page);
    uniqueBlockIds(expected);
    expect(expected.text).toContain("개인 기록 한글😀");
    expect((await other.page.request.get(`${target.path}/body`)).status()).toBe(404);
    await freshReadback(actors, owner, "owner", target, expected, info);
  });

  test(PRODUCT_TITLES[1], async ({ browser }, info) => {
    expect(browser).toBe(actors.browser);
    const owner = await actors.open("owner"),
      member = await actors.open("member"),
      outsider = await actors.open("outsider");
    const key = `F${randomUUID().replaceAll("-", "").slice(0, 6).toUpperCase()}`;
    const projects = `/api/v1/workspaces/${binding.workspaceId}/projects`;
    const projectRes = await owner.page.request.post(projects, {
      data: {
        key,
        name: "private selected final",
        visibility: "private",
      } satisfies Schema["CreateProjectBody"],
    });
    expect(projectRes.status()).toBe(201);
    const project = (await projectRes.json()) as Schema["ProjectOutput"];
    expect(project.rootDocumentId).toMatch(UUID_RE);
    const documents = `${projects}/${project.id}/documents`;
    const create = async (title: string) => {
      const res = await owner.page.request.post(documents, {
        data: {
          title,
          parentId: project.rootDocumentId,
        } satisfies Schema["CreateProjectDocumentBody"],
      });
      expect(res.status()).toBe(201);
      const document = (await res.json()) as Schema["DocumentMetaResponse"];
      expect(document.id).toMatch(UUID_RE);
      expect(document).toMatchObject({
        workspaceId: binding.workspaceId,
        projectId: project.id,
        parentId: project.rootDocumentId,
      });
      if (!document.displayId) throw new Error("SELECTED_FINAL_PROJECT_DISPLAY_ID_MISSING");
      return { ...document, displayId: document.displayId };
    };
    const doc = await create("project actual body"),
      reference = await create("selected 참조 대상");
    const target: Target = {
      id: doc.id,
      workspaceId: binding.workspaceId,
      path: `${documents}/${doc.id}`,
      url: `/w/${binding.workspaceSlug}/${doc.displayId}`,
    };
    for (const actor of [member, outsider])
      expect((await actor.page.request.get(`${target.path}/body`)).status()).toBe(404);
    await mount(owner, target, "on");
    const editor = owner.page.locator(".fvoci-editor .ProseMirror");
    await editor.click();
    await owner.page.keyboard.type("프로젝트 본문 한글😀");
    await owner.page.keyboard.press("Enter");
    await owner.page.keyboard.type(`/${reference.displayId}`);
    await owner.page
      .locator(".fvoci-suggestion")
      .getByRole("option", { name: `임베드 ${reference.displayId}`, exact: true })
      .click();
    await expect(owner.page.locator(".afn-embed-ref")).toHaveText("selected 참조 대상");
    const bytes = Buffer.from("project attachment bytes\n한글😀\n");
    await insertSlashAttachment(owner.page, {
      name: "selected-project.txt",
      buffer: bytes,
      mimeType: "text/plain",
    });
    await saveOn(owner);
    expect(await storedAttachmentDownloadBytes(owner.page)).toEqual(bytes);
    const beforeGrant = await editorShape(owner.page);
    const references: unknown[] = [];
    walkTiptap(beforeGrant.document, (node) => {
      if (node.type === "embed") references.push(node.attrs);
    });
    expect(references).toContainEqual(
      expect.objectContaining({ entity: "document", ref: reference.id }),
    );
    expect(attachmentNodes(beforeGrant)).toHaveLength(1);
    const attachment = attachmentNodes(beforeGrant)[0];
    if (!attachment) throw new Error("SELECTED_FINAL_ATTACHMENT_MISSING");
    expect(attachment.attachmentId).toMatch(UUID_RE);
    const grant = await owner.page.request.post(`${projects}/${project.id}/members`, {
      data: {
        userId: binding.member.userId,
        role: "member",
      } satisfies Schema["AddProjectMemberBody"],
    });
    expect(grant.status()).toBe(201);
    await mount(member, target, "on");
    await expectConverged(owner.page, member.page);
    const memberEditor = member.page.locator(".fvoci-editor .ProseMirror");
    await memberEditor.locator(":scope > p").last().click();
    await member.page.keyboard.press("End");
    await expect(memberEditor).toBeFocused();
    await member.page.keyboard.type(" 동료 입력");
    await expectTokens(owner.page, ["동료 입력"]);
    await saveOn(member);
    await expectConverged(owner.page, member.page);
    expect((await outsider.page.request.get(`${target.path}/body`)).status()).toBe(404);
    expect(
      (
        await outsider.page.request.get(
          `/api/v1/workspaces/${binding.workspaceId}/attachments/${attachment.attachmentId}/download`,
        )
      ).status(),
    ).toBe(404);
    await freshReadback(
      actors,
      member,
      "member",
      target,
      await editorShape(member.page),
      info,
      bytes,
    );
  });

  test(PRODUCT_TITLES[2], async ({ browser }, info) => {
    expect(browser).toBe(actors.browser);
    const owner = await actors.open("owner"),
      member = await actors.open("member");
    const target = await wiki(owner.page, binding, "current rights and live room");
    await mount(owner, target, "on");
    await owner.page.locator(".fvoci-editor .ProseMirror").click();
    await owner.page.keyboard.type("retained authorized history");
    await saveOn(owner);
    let closed = 0;
    member.page.on("websocket", (socket) => socket.on("close", () => closed++));
    await mount(member, target, "on");
    await expectConverged(owner.page, member.page);
    const membership = `/api/v1/workspaces/${binding.workspaceId}/members/${binding.member.userId}`;
    expect(
      (
        await owner.page.request.patch(membership, {
          data: { role: "guest" } satisfies Schema["MemberRoleBody"],
        })
      ).status(),
    ).toBe(200);
    await expect(
      member.page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]'),
    ).toHaveCount(0);
    await expect.poll(() => closed).toBeGreaterThan(0);
    const before = await body(owner.page, target);
    const history = await owner.page.request.get(`${target.path}/revisions`);
    expect(history.status()).toBe(200);
    const revisions: unknown = await history.json();
    // This is the normal authorized endpoint, not a fabricated browser ACK.
    const denied = async (contentMd: string) => {
      const data = { contentMd } satisfies Schema["PutDocumentBodyInput"];
      const response = await member.page.request.put(`${target.path}/body`, { data });
      expect(response.status()).toBe(404);
      actors.denials.push({
        targetId: target.id,
        workspaceId: target.workspaceId,
        actorId: binding.member.userId,
        method: "PUT",
        status: 404,
        operation: "body-put",
        payloadSha256: createHash("sha256").update(JSON.stringify(data)).digest("hex"),
      });
    };
    await denied("must never commit after role change");
    expect(await body(owner.page, target)).toEqual(before);
    const after = await owner.page.request.get(`${target.path}/revisions`);
    expect(after.status()).toBe(200);
    expect(await after.json()).toEqual(revisions);
    expect((await owner.page.request.delete(membership)).status()).toBe(200);
    await denied("must never commit after removal");
    expect(await body(owner.page, target)).toEqual(before);
    await owner.page.locator(".fvoci-editor .ProseMirror").click();
    await owner.page.keyboard.press("Control+End");
    await owner.page.keyboard.type(" healthy owner after refusal");
    await saveOn(owner);
    await freshReadback(actors, owner, "owner", target, await editorShape(owner.page), info);
  });

  test(PRODUCT_TITLES[3], async ({ browser }, info) => {
    expect(browser).toBe(actors.browser);
    const owner = await actors.open("owner"),
      peer = await actors.open("outsider");
    const target = await wiki(owner.page, binding, "same editor reconnect and personal undo");
    let offline = false;
    let socket: WebSocketRoute | undefined;
    await owner.page.routeWebSocket(/\/collab(?:\?|$)/, async (route) => {
      if (offline) {
        await route.close({ code: 1013, reason: "owned network pause" });
        return;
      }
      route.connectToServer();
      socket = route;
    });
    await mount(owner, target, "on");
    await mount(peer, target, "on");
    const editor = owner.page.locator(".fvoci-editor .ProseMirror");
    await editor.evaluate((root) => {
      (root as HTMLElement & { fvociProbe?: string }).fvociProbe = "same editor";
    });
    await editor.click();
    await owner.page.keyboard.type("살아남을한글😀");
    await saveOn(owner);
    await expectTokens(peer.page, ["살아남을한글😀"]);
    await owner.context.setOffline(true);
    try {
      if (!socket) throw new Error("SELECTED_FINAL_ACTUAL_SOCKET_MISSING");
      offline = true;
      await socket.close({ code: 1000, reason: "owned temporary disconnect" });
      await expect(owner.page.locator('[data-collab-status="connected"]')).toHaveCount(0);
      await editor.focus();
      await owner.page.keyboard.type(" 끊긴 동안 입력");
      await expectNotDurablySaved(owner.page);
      await expect(owner.page.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
    } finally {
      offline = false;
      await owner.context.setOffline(false);
    }
    await waitConnected(owner.page);
    await expectTokens(peer.page, ["끊긴 동안 입력"]);
    expect(
      await editor.evaluate((root) => (root as HTMLElement & { fvociProbe?: string }).fvociProbe),
    ).toBe("same editor");
    await expectConverged(owner.page, peer.page);
    await saveOn(owner);
    const retained = (await editorShape(owner.page)).text;
    await peer.page.locator(".fvoci-editor .ProseMirror").click();
    await peer.page.keyboard.press("Control+End");
    await peer.page.keyboard.type(" 동료 보존");
    await expectTokens(owner.page, ["동료 보존"]);
    await editor.click();
    await owner.page.keyboard.press("Control+End");
    await owner.page.keyboard.type(" 내 실행취소");
    await expectTokens(peer.page, ["내 실행취소"]);
    await owner.page.keyboard.press("Control+z");
    await expect
      .poll(async () => (await editorShape(owner.page)).text)
      .toBe(`${retained} 동료 보존`);
    await expectConverged(owner.page, peer.page);
    await owner.page.keyboard.press("Control+Shift+z");
    await expectTokens(peer.page, ["내 실행취소"]);
    await saveOn(owner);
    await insertSlashTable(owner.page);
    const bytes = Buffer.from("structured attachment bytes\n한글😀\n");
    await insertSlashAttachment(owner.page, {
      name: "selected-regression.txt",
      buffer: bytes,
      mimeType: "text/plain",
    });
    await saveOn(owner);
    const expected = await editorShape(owner.page);
    uniqueBlockIds(expected);
    expect(expected.table).not.toBeNull();
    expect(attachmentNodes(expected)).toHaveLength(1);
    expect(await storedAttachmentDownloadBytes(owner.page)).toEqual(bytes);
    await expectConverged(owner.page, peer.page);
    await freshReadback(actors, owner, "outsider", target, expected, info, bytes);
  });
} else {
  test(boundaryTitle(binding.browser, binding.flow), async ({ browser }, info) => {
    expect(browser).toBe(actors.browser);
    const owner = await actors.open("owner");
    const target = await wiki(owner.page, binding, "문서 경계");
    await mount(owner, target, binding.flow);
    await owner.page.locator(".fvoci-editor .ProseMirror").click();
    for (const [i, text] of ["첫 문단", "중간 문단", "끝 문단"].entries()) {
      if (i > 0) await owner.page.keyboard.press("Enter");
      await owner.page.keyboard.type(text);
    }
    if (binding.flow === "on") await saveOn(owner);
    else await saveOff(owner, target);
    await expectBlocks(owner.page, ["첫 문단", "중간 문단", "끝 문단"]);
    await expectDocumentBoundaries(owner.page, info);
    await owner.page.keyboard.press("Control+End");
    await owner.page.keyboard.type(" boundary");
    await expectBlocks(owner.page, ["첫 문단", "중간 문단", "끝 문단 boundary"]);
    if (binding.flow === "on") await saveOn(owner);
    else await saveOff(owner, target);
    await freshReadback(actors, owner, "owner", target, await editorShape(owner.page), info);
  });
}
