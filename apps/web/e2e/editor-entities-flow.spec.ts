import { expect, test, type APIRequestContext, type Locator, type Page } from "@playwright/test";
import { createE2eUser } from "./helpers";
import { admin, createDoc, editorOf, newSignedInPage, openDoc, save, setupInstance, workspaceId, type TiptapNode } from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });
const guest = { email: "entities-guest@example.com", password: "guestpass1", givenName: "참조손님" };

test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
  createE2eUser(guest.email, guest.password, guest.givenName, { workspaceSlug: admin.workspaceSlug, membershipRole: "guest" });
});

type Item = { id: string; number: number; title: string };
async function fixtures(request: APIRequestContext, key: string) {
  const ws = await workspaceId(request);
  const projectResponse = await request.post(`/api/v1/workspaces/${ws}/projects`, {
    data: { key, name: `${key} project`, visibility: "workspace" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = await projectResponse.json() as { id: string; rootDocumentId: string };
  const wiki = await createDoc(request, ws, `${key} wiki reference`);
  const documentResponse = await request.post(`/api/v1/workspaces/${ws}/projects/${project.id}/documents`, {
    data: { title: `${key} project reference`, parentId: project.rootDocumentId },
  });
  expect(documentResponse.status(), await documentResponse.text()).toBe(201);
  const document = await documentResponse.json() as Item;
  const taskResponse = await request.post(`/api/v1/workspaces/${ws}/projects/${project.id}/tasks`, {
    data: { title: `${key} task reference` },
  });
  expect(taskResponse.status(), await taskResponse.text()).toBe(201);
  const task = await taskResponse.json() as Item;
  return { ws, project, wiki, document, task };
}

async function nextParagraph(page: Page, editor: Locator) {
  // Clicking the editor's center can open the previous embed's textarea.
  // Target a paragraph so native typing/paste reaches the ProseMirror host.
  await editor.locator(":scope > p").last().click();
  await page.keyboard.press("End"); await page.keyboard.press("Enter");
  await expect(editor).toBeFocused();
}
async function mention(page: Page, query: string, title: string) {
  await page.keyboard.type(`@${query}`);
  await page.locator(".fvoci-suggestion").getByRole("option", { name: title, exact: true }).click();
}
async function slash(page: Page, displayId: string) {
  await page.keyboard.type(`/${displayId}`);
  await page.locator(".fvoci-suggestion").getByRole("option").filter({ hasText: displayId }).click();
}
async function paste(page: Page, ref: string) {
  const url = new URL(`/w/${admin.workspaceSlug}/${ref}`, page.url()).href;
  await page.evaluate(async (text) => navigator.clipboard.writeText(text), url);
  await page.keyboard.press("Control+V");
}
function nodes(body: TiptapNode, type: string): TiptapNode[] {
  return [...(body.type === type ? [body] : []), ...(body.content ?? []).flatMap((child) => nodes(child, type))];
}
async function body(request: APIRequestContext, path: string): Promise<TiptapNode> {
  const response = await request.get(path); expect(response.ok(), await response.text()).toBe(true);
  const data = await response.json(); return data.contentJson as TiptapNode;
}

for (const host of ["wiki", "project", "task"] as const) {
  test(`${host}: real @, slash, UUID/display paste persist and resolve after reload`, async ({ browser, baseURL }) => {
    const signed = await newSignedInPage(browser, baseURL, admin, { permissions: ["clipboard-read", "clipboard-write"] });
    const page = signed.page;
    try {
      const key = { wiki: "ENW", project: "ENP", task: "ENT" }[host];
      const f = await fixtures(page.request, key);
      if (host === "wiki") {
        const response = await page.request.post(`/api/v1/workspaces/${f.ws}/groups`, { data: { name: "Entity Team" } });
        expect(response.status(), await response.text()).toBe(201);
      }
      const path = host === "wiki" ? f.wiki.path : `/w/${admin.workspaceSlug}/${key}-${host === "project" ? f.document.number : f.task.number}`;
      const hostPath = path;
      const editor = await openDoc(page, hostPath);
      await expect(editor).toHaveAttribute("contenteditable", "true");
      await editor.click();
      await mention(page, `WIKI-${f.wiki.number}`, `${key} wiki reference`);
      await expect(editor.locator("[data-mention]")).toHaveText(`@${key} wiki reference`);
      await nextParagraph(page, editor);
      await slash(page, `${key}-${f.task.number}`);
      await expect(editor.locator('.afn-embed[data-entity="task"]')).toContainText(f.task.title);
      await nextParagraph(page, editor); await paste(page, f.document.id);
      await expect(editor.locator('.afn-embed[data-entity="document"]').first()).toContainText(f.document.title);
      await nextParagraph(page, editor); await paste(page, `WIKI-${f.wiki.number}`);
      await expect(editor.locator('.afn-embed[data-entity="document"]').last()).toContainText(`${key} wiki reference`);
      await save(page);
      const bodyPath = host === "wiki" ? `/api/v1/workspaces/${f.ws}/documents/${f.wiki.id}/body`
        : host === "project" ? `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents/${f.document.id}/body`
        : `/api/v1/workspaces/${f.ws}/tasks/${f.task.id}`;
      await expect.poll(async () => nodes(await body(page.request, bodyPath), "embed").length).toBe(3);
      const saved = await body(page.request, bodyPath);
      expect(nodes(saved, "mention")[0]!.attrs).toMatchObject({ entity: "document", id: f.wiki.id, label: `WIKI-${f.wiki.number}` });
      expect(nodes(saved, "embed").map((n) => ({ entity: n.attrs!.entity, ref: n.attrs!.ref }))).toEqual([
        { entity: "task", ref: f.task.id }, { entity: "document", ref: f.document.id }, { entity: "document", ref: `WIKI-${f.wiki.number}` },
      ]);
      if (host === "wiki") {
        const peer = await newSignedInPage(browser, baseURL, admin);
        try {
          const peerEditor = await openDoc(peer.page, hostPath);
          await expect(peerEditor.locator("[data-mention]")).toHaveText(`@${key} wiki reference`);
          await expect(peerEditor.locator(".afn-embed")).toHaveCount(3);
          await nextParagraph(page, editor); await mention(page, `${key}-${f.task.number}`, f.task.title);
          await expect(peerEditor.locator("[data-mention]").last()).toHaveText(`@${f.task.title}`);
          await nextParagraph(page, editor); await mention(page, "편집", "동료편집");
          await nextParagraph(page, editor); await mention(page, "Entity", "Entity Team");
          await expect(peerEditor.locator("[data-mention]").last()).toHaveText("@Entity Team");
          await save(page);
          const shared = await body(page.request, bodyPath);
          expect(nodes(shared, "mention").map((node) => node.attrs!.entity)).toEqual(["document", "task", "user", "group"]);
        } finally { await peer.context.close(); }
      }
      await page.reload();
      await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
      await expect(editorOf(page).locator('.afn-embed[data-entity="task"]')).toContainText(f.task.title);
      await expect(editorOf(page).locator('.afn-embed[data-entity="document"]').first()).toContainText(f.document.title);
      const reloaded = await body(page.request, bodyPath);
      expect(nodes(reloaded, "embed").map((n) => n.attrs)).toEqual(nodes(saved, "embed").map((n) => n.attrs));
    } finally { await signed.context.close(); }
  });
}

test("guest member denial keeps allowed entities; inaccessible refs and readonly retain permission boundaries", async ({ browser, baseURL }) => {
  const owner = await newSignedInPage(browser, baseURL, admin);
  const visitor = await newSignedInPage(browser, baseURL, guest, { permissions: ["clipboard-read", "clipboard-write"] });
  try {
    const f = await fixtures(owner.page.request, "ENG");
    const members = await owner.page.request.get(`/api/v1/workspaces/${f.ws}/members`);
    const guestId = (await members.json()).items.find((m: { email: string }) => m.email === guest.email).userId;
    const grant = `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/members`;
    expect((await owner.page.request.post(grant, { data: { userId: guestId, role: "member" } })).ok()).toBe(true);
    const page = visitor.page;
    const editor = await openDoc(page, `/w/${admin.workspaceSlug}/ENG-${f.document.number}`);
    expect((await page.request.get(`/api/v1/workspaces/${f.ws}/members`)).status()).toBe(404);
    await editor.click(); await mention(page, `ENG-${f.task.number}`, f.task.title);
    await expect(editor.locator("[data-mention]")).toHaveText(`@${f.task.title}`);
    await nextParagraph(page, editor); await paste(page, f.wiki.id);
    await expect(editor.locator(".afn-embed-inaccessible")).toBeVisible();
    await expect(editor).not.toContainText("ENG wiki reference");
    await nextParagraph(page, editor); await page.keyboard.type("/");
    await expect(page.locator(".fvoci-suggestion").getByRole("option").first()).toBeVisible();
    await page.keyboard.press("Escape"); await page.keyboard.press("Backspace"); await save(page);
    // Revoke editing through the real project membership API, then revalidate.
    expect((await owner.page.request.patch(`${grant}/${guestId}`, { data: { role: "viewer" } })).ok()).toBe(true);
    await page.reload();
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
    await expect(editorOf(page)).toHaveAttribute("contenteditable", "false");
    await expect(editorOf(page).locator(".afn-embed-inaccessible")).toBeVisible();
    const before = await body(page.request, `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents/${f.document.id}/body`);
    await editorOf(page).click(); await page.keyboard.type("@ENG-1");
    expect(await body(page.request, `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents/${f.document.id}/body`)).toEqual(before);
    await expect(page.locator(".fvoci-suggestion")).toHaveCount(0);
  } finally { await visitor.context.close(); await owner.context.close(); }
});
