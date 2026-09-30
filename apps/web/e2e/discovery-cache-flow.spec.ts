import { expect, test, type Page } from "@playwright/test";
import { admin, createDoc, newSignedInPage, setupInstance, workspaceId } from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });
test.beforeAll(async ({ browser, baseURL }) => setupInstance(browser, baseURL));

// Keep the mounted application's actual QueryClient through every navigation.
// A document reload or forced invalidation would hide this regression.
async function push(page: Page, path: string) {
  await page.evaluate(async (path) => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: { config: { globalProperties: { $router: { push: (path: string) => Promise<unknown> } } } };
    };
    await root.__vue_app__.config.globalProperties.$router.push(path);
  }, path);
  await expect(page).toHaveURL(new RegExp(`${path.replace(/[?]/g, "\\?")}$`));
}
async function discoverySnapshot(page: Page, ws: string, tag = "") {
  return page.evaluate(({ ws, tag }) => {
    type Client = import("@tanstack/vue-query").QueryClient;
    const root = document.getElementById("root") as HTMLElement & { __vue_app__: { _context: { provides: Record<string, Client> } } };
    const client = root.__vue_app__._context.provides.VUE_QUERY_CLIENT!;
    const state = client.getQueryState(["wiki-discovery", ws, tag]);
    return { age: Date.now() - (state?.dataUpdatedAt ?? 0), updatedAt: state?.dataUpdatedAt, invalidated: state?.isInvalidated,
      countsUpdatedAt: client.getQueryState(["me", "workspaces"])?.dataUpdatedAt,
      workspaceCount: client.getQueryData<{ items: { id: string; documentCount: number }[] }>(["me", "workspaces"])?.items.find(item => item.id === ws)?.documentCount };
  }, { ws, tag });
}

for (const host of ["wiki", "project"] as const) {
  test(`${host}: metadata and tag membership refresh retained discovery before 30s`, async ({ browser, baseURL }) => {
    const signed = await newSignedInPage(browser, baseURL, admin); const page = signed.page;
    try {
      const ws = await workspaceId(page.request); const key = host === "wiki" ? "DCW" : "DCP";
      let doc: { path: string; number: number; id: string }; let prefix: string;
      if (host === "wiki") {
        doc = await createDoc(page.request, ws, `${key} original`); prefix = `/api/v1/workspaces/${ws}/documents/${doc.id}`;
      } else {
        const response = await page.request.post(`/api/v1/workspaces/${ws}/projects`, { data: { key, name: `${key} project`, visibility: "workspace" } });
        expect(response.status()).toBe(201); const project = await response.json();
        const created = await page.request.post(`/api/v1/workspaces/${ws}/projects/${project.id}/documents`, { data: { parentId: project.rootDocumentId, title: `${key} original` } });
        expect(created.status()).toBe(201); const data = await created.json();
        doc = { ...data, path: `/w/${admin.workspaceSlug}/${key}-${data.number}` }; prefix = `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${doc.id}`;
      }
      const tagResponse = await page.request.post(`/api/v1/workspaces/${ws}/document-tags`, { data: { name: `${key} tag`, color: "gray" } });
      expect(tagResponse.status()).toBe(201); const tag = await tagResponse.json();
      const list = `/w/${admin.workspaceSlug}/wiki`; const ref = host === "wiki" ? `WIKI-${doc.number}` : `${key}-${doc.number}`;
      await push(page, list); await expect(page.getByTestId(`wiki-doc-${ref}`)).toContainText(`${key} original`);
      await push(page, `${list}?tag=${tag.id}`); await expect(page.getByTestId(`wiki-doc-${ref}`)).toHaveCount(0);
      await expect.poll(async () => (await discoverySnapshot(page, ws, tag.id)).age).toBeLessThan(30_000);
      const cached = await discoverySnapshot(page, ws);
      await push(page, doc.path); await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
      const title = page.locator(".document-page__title");
      await title.fill(`${key} renamed`);
      let mutation = page.waitForResponse(r => r.url().endsWith(prefix) && r.request().method() === "PATCH" && r.ok());
      await title.press("Tab"); await mutation; await expect(title).toBeEnabled();
      const icon = page.locator(host === "wiki" ? "#document-icon" : "#project-document-icon");
      await icon.fill("📘"); mutation = page.waitForResponse(r => r.url().endsWith(prefix) && r.request().method() === "PATCH" && r.ok());
      await icon.press("Tab"); await mutation; await expect(icon).toBeEnabled();
      mutation = page.waitForResponse(r => r.url().endsWith(prefix) && r.request().method() === "PATCH" && r.ok());
      await page.locator(host === "wiki" ? "#document-status" : "#project-document-status").selectOption("published"); await mutation;
      await expect.poll(async () => (await discoverySnapshot(page, ws)).invalidated).toBe(true);
      const bar = page.getByTestId("document-tags-bar");
      await bar.getByRole("button", { name: "+ 태그", exact: true }).click();
      await bar.getByRole("button", { name: `${key} tag`, exact: true }).click();
      await expect(bar.locator(".tags-bar__item")).toContainText(`${key} tag`);
      await expect(bar.locator(".tags-bar__picker")).toHaveCount(0);
      expect((await discoverySnapshot(page, ws)).countsUpdatedAt).toBe(cached.countsUpdatedAt);
      expect((await discoverySnapshot(page, ws)).age).toBeLessThan(30_000);
      await push(page, list);
      const row = page.getByTestId(`wiki-doc-${ref}`);
      await expect(row).toContainText(`${key} renamed`); await expect(row.locator(".wiki-tree__icon")).toHaveText("📘");
      await expect(row.locator(".wiki-tree__status")).toHaveCount(0);
      await push(page, `${list}?tag=${tag.id}`); await expect(row).toContainText(`${key} renamed`);
      const filtered = await discoverySnapshot(page, ws, tag.id);
      await push(page, doc.path); await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
      await bar.locator(".tags-bar__remove").click(); await expect(bar.locator(".tags-bar__item")).toHaveCount(0);
      await expect.poll(async () => (await discoverySnapshot(page, ws, tag.id)).invalidated).toBe(true);
      expect((await discoverySnapshot(page, ws, tag.id)).age).toBeLessThan(30_000);
      await push(page, `${list}?tag=${tag.id}`); await expect(row).toHaveCount(0);
      expect((await discoverySnapshot(page, ws, tag.id)).updatedAt).toBeGreaterThan(filtered.updatedAt!);
    } finally { await signed.context.close(); }
  });
}

test("project restore refreshes retained discovery, project lists and workspace counts before 30s", async ({ browser, baseURL }) => {
  const signed = await newSignedInPage(browser, baseURL, admin); const page = signed.page;
  try {
    const ws = await workspaceId(page.request); const prefix = `/api/v1/workspaces/${ws}/projects`;
    const created = await page.request.post(prefix, { data: { key: "DCR", name: "Discovery restored project", visibility: "workspace" } });
    expect(created.status()).toBe(201); const project = await created.json();
    const docResponse = await page.request.post(`${prefix}/${project.id}/documents`, { data: { title: "Discovery restored document", parentId: project.rootDocumentId } });
    expect(docResponse.status()).toBe(201); const doc = await docResponse.json();
    const tagResponse = await page.request.post(`/api/v1/workspaces/${ws}/document-tags`, { data: { name: "Restore discovery tag", color: "gray" } });
    expect(tagResponse.status()).toBe(201); const tag = await tagResponse.json();
    expect((await page.request.post(`${prefix}/${project.id}/documents/${doc.id}/tags`, { data: { tagId: tag.id } })).status()).toBe(200);
    expect((await page.request.delete(`${prefix}/${project.id}`)).ok()).toBe(true);
    const list = `/w/${admin.workspaceSlug}/wiki`; const home = `/w/${admin.workspaceSlug}`;
    await push(page, home); await expect(page.getByTestId("workspace-totals")).toBeVisible();
    const countsBefore = await page.getByTestId("workspace-totals").innerText();
    await push(page, list); await expect(page.getByTestId(`wiki-doc-DCR-${doc.number}`)).toHaveCount(0);
    await expect.poll(async () => (await discoverySnapshot(page, ws)).age).toBeLessThan(30_000);
    const before = await discoverySnapshot(page, ws);
    await push(page, `${list}?tag=${tag.id}`);
    await expect(page.getByTestId(`wiki-doc-DCR-${doc.number}`)).toHaveCount(0);
    await expect.poll(async () => (await discoverySnapshot(page, ws, tag.id)).age).toBeLessThan(30_000);
    await push(page, `/w/${admin.workspaceSlug}/settings`);
    await page.getByTestId("deleted-projects").getByRole("button", { name: "복원 Discovery restored project" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "복원", exact: true }).click();
    await expect(page.getByTestId("deleted-projects")).toHaveCount(0);
    expect((await discoverySnapshot(page, ws)).age).toBeLessThan(30_000);
    await push(page, list); await expect(page.getByTestId(`wiki-doc-DCR-${doc.number}`)).toContainText("Discovery restored document");
    expect((await discoverySnapshot(page, ws)).updatedAt).toBeGreaterThan(before.updatedAt!);
    await push(page, `${list}?tag=${tag.id}`); await expect(page.getByTestId(`wiki-doc-DCR-${doc.number}`)).toContainText("Discovery restored document");
    await push(page, home); await expect(page.getByText("Discovery restored project", { exact: true })).toBeVisible();
    await expect.poll(async () => (await discoverySnapshot(page, ws)).workspaceCount).toBe(before.workspaceCount! + 2);
    await expect(page.getByTestId("workspace-totals")).not.toHaveText(countsBefore);
    const active = await (await page.request.get(prefix)).json(); expect(active.items.some((p: { id: string }) => p.id === project.id)).toBe(true);
  } finally { await signed.context.close(); }
});
