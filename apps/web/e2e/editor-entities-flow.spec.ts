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

// Delay delivery, not execution: every held response comes from the real Rust
// server/DB. Edits and peer presence then replace the computed session while
// the document lifecycle mutation is still waiting for its HTTP response.
async function holdResponse(page: Page, path: string, deferRequest = false) {
  let forward!: () => void;
  const requestGate = new Promise<void>((resolve) => { forward = resolve; });
  if (!deferRequest) forward();
  let release!: () => void;
  const wait = new Promise<void>((resolve) => { release = resolve; });
  let received!: (status: number) => void;
  const response = new Promise<number>((resolve) => { received = resolve; });
  await page.route(`**${path}`, async (route) => {
    await requestGate;
    const result = await route.fetch();
    received(result.status());
    await wait;
    await route.fulfill({ response: result });
  }, { times: 1 });
  return { response, release: () => { forward(); release(); }, forward };
}

async function retainedCounts(page: Page, ws: string, refresh = false) {
  // Inspect the existing app's actual QueryClient; no product test hook or
  // replacement client. fetchQuery must honor the retained 30-second cache.
  return page.evaluate(async ({ workspaceId, refresh }) => {
    type Client = import("@tanstack/vue-query").QueryClient;
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: { _context: { provides: Record<string, Client> } };
    };
    const client = root.__vue_app__._context.provides.VUE_QUERY_CLIENT!;
    const counts = await client.fetchQuery({
      queryKey: ["me", "workspaces"], staleTime: refresh ? 0 : 30_000,
      queryFn: async () => {
        const response = await fetch("/api/v1/me/workspaces");
        if (!response.ok) throw new Error(`workspace counts ${response.status}`);
        return response.json() as Promise<{ items: { id: string; documentCount: number }[] }>;
      },
    });
    await client.fetchQuery({
      queryKey: ["projects", workspaceId], staleTime: 30_000,
      queryFn: async () => {
        const response = await fetch(`/api/v1/workspaces/${workspaceId}/projects`);
        if (!response.ok) throw new Error(`project counts ${response.status}`);
        return response.json();
      },
    });
    return counts.items.find((item) => item.id === workspaceId)!.documentCount;
  }, { workspaceId: ws, refresh });
}
async function pushDocument(page: Page, path: string) {
  await page.evaluate(async (next) => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: { config: { globalProperties: { $router: { push: (path: string) => Promise<unknown> } } } };
    };
    await root.__vue_app__.config.globalProperties.$router.push(next);
  }, path);
  await expect(page).toHaveURL(new RegExp(`${path}$`));
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(editorOf(page)).toBeVisible();
}

for (const host of ["wiki", "project"] as const) {
  test(`${host}: real delayed move/trash survives same-room ACKs and retires on a room switch`, async ({ browser, baseURL }) => {
    const signed = await newSignedInPage(browser, baseURL, admin);
    const peer = await newSignedInPage(browser, baseURL, admin);
    const page = signed.page;
    const held: { release: () => void }[] = [];
    try {
      const key = host === "wiki" ? "LCW" : "LCP";
      const f = await fixtures(page.request, key);
      async function extra(title: string) {
        if (host === "wiki") return createDoc(page.request, f.ws, title);
        const response = await page.request.post(`/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents`, {
          data: { title, parentId: f.project.rootDocumentId },
        });
        expect(response.status(), await response.text()).toBe(201);
        const document = await response.json() as Item;
        return { ...document, path: `/w/${admin.workspaceSlug}/${key}-${document.number}` };
      }
      const parent = await extra(`${key} parent`);
      const retired = await extra(`${key} retired parent`);
      const next = await extra(`${key} next room`);
      const original = host === "wiki" ? f.wiki : { ...f.document, path: `/w/${admin.workspaceSlug}/${key}-${f.document.number}` };
      const prefix = host === "wiki" ? `/api/v1/workspaces/${f.ws}/documents`
        : `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents`;
      await openDoc(page, original.path);
      const countBefore = await retainedCounts(page, f.ws);
      let workspaceRefreshes = 0; let projectRefreshes = 0;
      page.on("request", (request) => {
        if (request.method() !== "GET") return;
        if (request.url().endsWith("/api/v1/me/workspaces")) workspaceRefreshes += 1;
        if (request.url().endsWith(`/api/v1/workspaces/${f.ws}/projects`)) projectRefreshes += 1;
      });
      const parentSelect = page.getByLabel("새 위치(부모 문서)");
      await parentSelect.selectOption(parent.id);
      const moved = await holdResponse(page, `${prefix}/${original.id}/move`); held.push(moved);
      await page.getByRole("button", { name: "이동", exact: true }).click();
      expect(await moved.response).toBe(200);
      await openDoc(peer.page, original.path);
      await editorOf(page).click(); await page.keyboard.type(`${key} edit during move`); await save(page);
      await expect(editorOf(peer.page)).toContainText(`${key} edit during move`);
      moved.release();
      await expect(parentSelect).toHaveValue("");
      const movedMeta = await page.request.get(`${prefix}/${original.id}`);
      expect((await movedMeta.json()).parentId).toBe(parent.id);
      // Both real queries already have fresh 30-second cache data. The
      // active workspace query refetches; inactive project data becomes stale.
      expect(workspaceRefreshes).toBeGreaterThan(0);
      const projectInvalidated = await page.evaluate((workspaceId) => {
        type Client = import("@tanstack/vue-query").QueryClient;
        const root = document.getElementById("root") as HTMLElement & { __vue_app__: { _context: { provides: Record<string, Client> } } };
        return root.__vue_app__._context.provides.VUE_QUERY_CLIENT!.getQueryState(["projects", workspaceId])?.isInvalidated;
      }, f.ws);
      expect(projectRefreshes > 0 || projectInvalidated).toBe(true);
      expect(await retainedCounts(page, f.ws)).toBe(countBefore);

      // A real missing-parent failure still appears after a successful persist
      // ACK. The browser retains the option; the API independently trashes it.
      await parentSelect.selectOption(retired.id);
      expect((await page.request.post(`${prefix}/${retired.id}/trash`)).ok()).toBe(true);
      const failed = await holdResponse(page, `${prefix}/${original.id}/move`); held.push(failed);
      await page.getByRole("button", { name: "이동", exact: true }).click();
      expect(await failed.response).toBeGreaterThanOrEqual(400);
      await editorOf(page).click(); await page.keyboard.press("End"); await page.keyboard.type(" error ACK"); await save(page);
      failed.release();
      await expect(page.locator(".document-page__error[role=alert]")).toBeVisible();

      // Retire a real in-flight error by switching via the actual SPA router.
      const lateError = await holdResponse(page, `${prefix}/${original.id}/move`); held.push(lateError);
      await page.getByRole("button", { name: "이동", exact: true }).click();
      expect(await lateError.response).toBeGreaterThanOrEqual(400);
      await pushDocument(page, next.path);
      const deliveredError = page.waitForResponse((response) => response.url().endsWith(`${prefix}/${original.id}/move`));
      lateError.release(); await deliveredError;
      await expect(page.locator(".document-page__error[role=alert]")).toHaveCount(0);

      // Retire successful trash: cache effects still apply to the old scope,
      // but its completion must never redirect the newly opened room.
      await pushDocument(page, original.path);
      // The earlier API-only fixture deletion bypassed browser invalidation.
      // Refresh the real baseline once, then retain it through the room switch.
      const beforeTrash = await retainedCounts(page, f.ws, true);
      const lateTrash = await holdResponse(page, `${prefix}/${original.id}/trash`); held.push(lateTrash);
      page.once("dialog", (dialog) => dialog.accept());
      await page.getByRole("button", { name: "휴지통으로 이동", exact: true }).click();
      expect(await lateTrash.response).toBe(200);
      await pushDocument(page, next.path);
      const deliveredTrash = page.waitForResponse((response) => response.url().endsWith(`${prefix}/${original.id}/trash`));
      lateTrash.release(); await deliveredTrash;
      await expect.poll(() => retainedCounts(page, f.ws)).toBe(beforeTrash - 1);
      await expect(page).toHaveURL(new RegExp(`${next.path}$`));
      await expect(page.getByLabel("문서 제목")).toHaveValue(`${key} next room`);

      // Current-room trash gets a persist ACK before delivery and must navigate.
      await openDoc(peer.page, next.path);
      const currentTrash = await holdResponse(page, `${prefix}/${next.id}/trash`, true); held.push(currentTrash);
      // Deleting a document may close its collab room. Hold the outgoing
      // request until the captured operation has observed a real persist ACK.
      page.once("dialog", (dialog) => dialog.accept());
      await page.getByRole("button", { name: "휴지통으로 이동", exact: true }).click();
      await editorOf(page).click(); await page.keyboard.type("during trash ACK"); await save(page);
      await expect(editorOf(peer.page)).toContainText("during trash ACK");
      currentTrash.forward(); expect(await currentTrash.response).toBe(200);
      currentTrash.release();
      await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/trash$`));
      const counts = await page.request.get("/api/v1/me/workspaces");
      expect((await counts.json()).items.find((item: { id: string }) => item.id === f.ws).documentCount).toBe(beforeTrash - 2);
    } finally {
      for (const response of held) response.release();
      await peer.context.close(); await signed.context.close();
    }
  });
}
