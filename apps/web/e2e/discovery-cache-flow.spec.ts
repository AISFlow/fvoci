import { readJson, flowSchemas, login } from "./helpers";
import { buildCacheObserverFixture } from "./cache-observer-fixture";
import type { CacheObservationWindow } from "./cache-observer-adapter";
import { expect, test, type Page, type Browser } from "@playwright/test";
import { admin, createDoc, setupInstance, workspaceId } from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });
let fixture: ReturnType<typeof buildCacheObserverFixture> | undefined;
test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
  fixture = buildCacheObserverFixture();
});
test.afterAll(() => {
  fixture?.dispose();
});
test.afterEach(async () => {
  if (fixture)
    await test.info().attach("cache-observer-served-assets", {
      body: Buffer.from(JSON.stringify(await fixture.evidence(), null, 2)),
      contentType: "application/json",
    });
});
async function newSignedInPage(browser: Browser, baseURL: string | undefined, who: typeof admin) {
  const context = await browser.newContext({ baseURL });
  if (!fixture) throw new Error("Cache observation fixture was not built");
  fixture.install(context);
  const page = await context.newPage();
  await login(page, who.email, who.password);
  return { context, page };
}

// Keep the mounted application's actual QueryClient through every navigation.
// A document reload or forced invalidation would hide this regression.
async function push(page: Page, path: string) {
  const mountedAt = await page.evaluate(() => performance.timeOrigin);
  await expect.poll(() => page.evaluate(() => "fvociCacheObservationFixture" in window)).toBe(true);
  await page.evaluate(async (path) => {
    await (window as CacheObservationWindow).fvociCacheObservationFixture.push(path);
  }, path);
  await expect(page).toHaveURL(new RegExp(`${path.replace(/[?]/g, "\\?")}$`));
  expect(await page.evaluate(() => performance.timeOrigin)).toBe(mountedAt);
}
async function discoverySnapshot(page: Page, ws: string, tag = "") {
  const snapshot = await page.evaluate(
    ({ ws, tag }) =>
      (window as CacheObservationWindow).fvociCacheObservationFixture.discoverySnapshot(ws, tag),
    { ws, tag },
  );
  expect(snapshot.staleTime).toBe(30_000);
  return snapshot;
}

for (const host of ["wiki", "project"] as const) {
  test(`${host}: metadata and tag membership refresh retained discovery before 30s`, async ({
    browser,
    baseURL,
  }) => {
    const signed = await newSignedInPage(browser, baseURL, admin);
    const page = signed.page;
    try {
      const ws = await workspaceId(page.request);
      const key = host === "wiki" ? "DCW" : "DCP";
      let doc: {
        path: string;
        number: number;
        id: string;
      };
      let prefix: string;
      if (host === "wiki") {
        doc = await createDoc(page.request, ws, `${key} original`);
        prefix = `/api/v1/workspaces/${ws}/documents/${doc.id}`;
      } else {
        const response = await page.request.post(`/api/v1/workspaces/${ws}/projects`, {
          data: { key, name: `${key} project`, visibility: "workspace" },
        });
        expect(response.status()).toBe(201);
        const project = await readJson(response, flowSchemas.project);
        const created = await page.request.post(
          `/api/v1/workspaces/${ws}/projects/${project.id}/documents`,
          { data: { parentId: project.rootDocumentId, title: `${key} original` } },
        );
        expect(created.status()).toBe(201);
        const data = await readJson(created, flowSchemas.document);
        doc = { ...data, path: `/w/${admin.workspaceSlug}/${key}-${String(data.number)}` };
        prefix = `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${doc.id}`;
      }
      const tagResponse = await page.request.post(`/api/v1/workspaces/${ws}/document-tags`, {
        data: { name: `${key} tag`, color: "gray" },
      });
      expect(tagResponse.status()).toBe(201);
      const tag = await readJson(tagResponse, flowSchemas.tag);
      const list = `/w/${admin.workspaceSlug}/wiki`;
      const ref = host === "wiki" ? `WIKI-${String(doc.number)}` : `${key}-${String(doc.number)}`;
      await push(page, list);
      await expect(page.getByTestId(`wiki-doc-${ref}`)).toContainText(`${key} original`);
      await push(page, `${list}?tag=${tag.id}`);
      await expect(page.getByTestId(`wiki-doc-${ref}`)).toHaveCount(0);
      await expect
        .poll(async () => (await discoverySnapshot(page, ws, tag.id)).age)
        .toBeLessThan(30000);
      const cached = await discoverySnapshot(page, ws);
      await push(page, doc.path);
      await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({
        timeout: 15000,
      });
      const title = page.locator(".document-page__title");
      await title.fill(`${key} renamed`);
      let mutation = page.waitForResponse(
        (r) => r.url().endsWith(prefix) && r.request().method() === "PATCH" && r.ok(),
      );
      await title.press("Tab");
      await mutation;
      await expect(title).toBeEnabled();
      await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
      const icon = page.locator(host === "wiki" ? "#document-icon" : "#project-document-icon");
      await icon.fill("📘");
      mutation = page.waitForResponse(
        (r) => r.url().endsWith(prefix) && r.request().method() === "PATCH" && r.ok(),
      );
      await icon.press("Tab");
      await mutation;
      await expect(icon).toBeEnabled();
      mutation = page.waitForResponse(
        (r) => r.url().endsWith(prefix) && r.request().method() === "PATCH" && r.ok(),
      );
      await page
        .locator(host === "wiki" ? "#document-status" : "#project-document-status")
        .selectOption("published");
      await mutation;
      await expect.poll(async () => (await discoverySnapshot(page, ws)).invalidated).toBe(true);
      const bar = page.getByTestId("document-tags-bar");
      await bar.getByRole("button", { name: "+ 태그", exact: true }).click();
      await bar.getByRole("button", { name: `${key} tag`, exact: true }).click();
      await expect(bar.locator(".tags-bar__item")).toContainText(`${key} tag`);
      await expect(bar.locator(".tags-bar__picker")).toHaveCount(0);
      expect((await discoverySnapshot(page, ws)).countsUpdatedAt).toBe(cached.countsUpdatedAt);
      expect((await discoverySnapshot(page, ws)).age).toBeLessThan(30000);
      await push(page, list);
      const row = page.getByTestId(`wiki-doc-${ref}`);
      await expect(row).toContainText(`${key} renamed`);
      await expect(row.locator(".wiki-tree__icon")).toHaveText("📘");
      await expect(row.locator(".wiki-tree__status")).toHaveCount(0);
      await push(page, `${list}?tag=${tag.id}`);
      await expect(row).toContainText(`${key} renamed`);
      const filtered = await discoverySnapshot(page, ws, tag.id);
      await push(page, doc.path);
      await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({
        timeout: 15000,
      });
      await bar.locator(".tags-bar__remove").click();
      await expect(bar.locator(".tags-bar__item")).toHaveCount(0);
      await expect
        .poll(async () => (await discoverySnapshot(page, ws, tag.id)).invalidated)
        .toBe(true);
      expect((await discoverySnapshot(page, ws, tag.id)).age).toBeLessThan(30000);
      await push(page, `${list}?tag=${tag.id}`);
      await expect(row).toHaveCount(0);
      const required2 = filtered.updatedAt;
      if (required2 === undefined) {
        throw new Error("Missing fixture value: filtered.updatedAt");
      }
      expect((await discoverySnapshot(page, ws, tag.id)).updatedAt).toBeGreaterThan(required2);
    } finally {
      await signed.context.close();
    }
  });
}

test("project restore refreshes retained discovery, project lists and workspace counts before 30s", async ({
  browser,
  baseURL,
}) => {
  // Prepare the fixture before mounting the app: HomePage fetches workspace
  // totals on login, which can otherwise cache the intermediate root-only count.
  const context = await browser.newContext({ baseURL });
  if (!fixture) throw new Error("Cache observation fixture was not built");
  fixture.install(context);
  const page = await context.newPage();
  try {
    expect(
      (
        await page.request.post("/api/v1/auth/login", {
          data: { email: admin.email, password: admin.password },
        })
      ).status(),
    ).toBe(200);
    const ws = await workspaceId(page.request);
    const workspaceCount = async () => {
      const response = await page.request.get("/api/v1/me/workspaces");
      expect(response.status()).toBe(200);
      const data = await readJson(response, flowSchemas.workspaces);
      const fixtureValue1 = data.items.find((item: { id: string }) => item.id === ws);
      if (fixtureValue1 === undefined)
        throw new Error(
          "Missing fixture value: data.items.find((item: { id: string }) => item.id === ws)",
        );
      return fixtureValue1.documentCount;
    };
    const deletedCount = await workspaceCount();
    const prefix = `/api/v1/workspaces/${ws}/projects`;
    const created = await page.request.post(prefix, {
      data: { key: "DCR", name: "Discovery restored project", visibility: "workspace" },
    });
    expect(created.status()).toBe(201);
    const project = await readJson(created, flowSchemas.project);
    const docResponse = await page.request.post(`${prefix}/${project.id}/documents`, {
      data: { title: "Discovery restored document", parentId: project.rootDocumentId },
    });
    expect(docResponse.status()).toBe(201);
    const doc = await readJson(docResponse, flowSchemas.document);
    const tagResponse = await page.request.post(`/api/v1/workspaces/${ws}/document-tags`, {
      data: { name: "Restore discovery tag", color: "gray" },
    });
    expect(tagResponse.status()).toBe(201);
    const tag = await readJson(tagResponse, flowSchemas.tag);
    expect(
      (
        await page.request.post(`${prefix}/${project.id}/documents/${doc.id}/tags`, {
          data: { tagId: tag.id },
        })
      ).status(),
    ).toBe(200);
    expect(await workspaceCount()).toBe(deletedCount + 2); // Project root + child.
    expect((await page.request.delete(`${prefix}/${project.id}`)).ok()).toBe(true);
    expect(await workspaceCount()).toBe(deletedCount);
    const list = `/w/${admin.workspaceSlug}/wiki`;
    const home = `/w/${admin.workspaceSlug}`;
    await page.goto(home);
    await expect(page.getByTestId("workspace-totals")).toBeVisible();
    const countsBefore = await page.getByTestId("workspace-totals").innerText();
    await push(page, list);
    await expect(page.getByTestId(`wiki-doc-DCR-${String(doc.number)}`)).toHaveCount(0);
    await expect.poll(async () => (await discoverySnapshot(page, ws)).age).toBeLessThan(30000);
    const before = await discoverySnapshot(page, ws);
    expect(before.workspaceCount).toBe(deletedCount);
    await push(page, `${list}?tag=${tag.id}`);
    await expect(page.getByTestId(`wiki-doc-DCR-${String(doc.number)}`)).toHaveCount(0);
    await expect
      .poll(async () => (await discoverySnapshot(page, ws, tag.id)).age)
      .toBeLessThan(30000);
    await push(page, `/w/${admin.workspaceSlug}/settings`);
    await page
      .getByTestId("deleted-projects")
      .getByRole("button", { name: "복원 Discovery restored project" })
      .click();
    await page.getByRole("dialog").getByRole("button", { name: "복원", exact: true }).click();
    await expect(page.getByTestId("deleted-projects")).toHaveCount(0);
    expect((await discoverySnapshot(page, ws)).age).toBeLessThan(30000);
    await push(page, list);
    await expect(page.getByTestId(`wiki-doc-DCR-${String(doc.number)}`)).toContainText(
      "Discovery restored document",
    );
    const required3 = before.updatedAt;
    if (required3 === undefined) {
      throw new Error("Missing fixture value: before.updatedAt");
    }
    expect((await discoverySnapshot(page, ws)).updatedAt).toBeGreaterThan(required3);
    await push(page, `${list}?tag=${tag.id}`);
    await expect(page.getByTestId(`wiki-doc-DCR-${String(doc.number)}`)).toContainText(
      "Discovery restored document",
    );
    await push(page, home);
    await expect(page.getByText("Discovery restored project", { exact: true })).toBeVisible();
    const required4 = before.workspaceCount;
    if (required4 === undefined) {
      throw new Error("Missing fixture value: before.workspaceCount");
    }
    expect(await workspaceCount()).toBe(required4 + 2);
    const required5 = before.workspaceCount;
    if (required5 === undefined) {
      throw new Error("Missing fixture value: before.workspaceCount");
    }
    await expect
      .poll(async () => (await discoverySnapshot(page, ws)).workspaceCount)
      .toBe(required5 + 2);
    const required6 = before.countsUpdatedAt;
    if (required6 === undefined) {
      throw new Error("Missing fixture value: before.countsUpdatedAt");
    }
    expect((await discoverySnapshot(page, ws)).countsUpdatedAt).toBeGreaterThan(required6);
    await expect(page.getByTestId("workspace-totals")).not.toHaveText(countsBefore);
    const active = await readJson(await page.request.get(prefix), flowSchemas.projects);
    expect(active.items.some((p: { id: string }) => p.id === project.id)).toBe(true);
  } finally {
    await context.close();
  }
});
