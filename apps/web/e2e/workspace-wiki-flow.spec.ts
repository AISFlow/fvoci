import { expect, test, type Page, type Response, type Route } from "@playwright/test";
import { createE2eUser, login, logout } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
};

const member = {
  email: "wiki-member@example.com",
  password: "memberpass1",
  givenName: "위키",
  familyName: "멤버",
};

const guest = {
  email: "wiki-guest@example.com",
  password: "guestpass1",
  givenName: "게스트",
  familyName: "위키",
};

function documentResourcePath(workspaceId: string, documentId: string): string {
  return `/api/v1/workspaces/${workspaceId}/documents/${documentId}`;
}

function patchBodyHas(
  request: { postDataJSON: () => unknown },
  expected: Record<string, unknown>,
): boolean {
  try {
    const body = request.postDataJSON();
    if (body === null || typeof body !== "object") return false;
    const record = body as Record<string, unknown>;
    return Object.entries(expected).every(([key, value]) => Object.is(record[key], value));
  } catch {
    return false;
  }
}

function isMatchingDocumentPatch(
  response: Response,
  workspaceId: string,
  documentId: string,
  expected: Record<string, unknown>,
): boolean {
  const request = response.request();
  return (
    request.method() === "PATCH" &&
    response.ok() &&
    new URL(response.url()).pathname === documentResourcePath(workspaceId, documentId) &&
    patchBodyHas(request, expected)
  );
}

async function persistedDocument(
  page: Page,
  workspaceId: string,
  documentId: string,
): Promise<{ title: string; icon: string | null; status: string }> {
  const res = await page.request.get(documentResourcePath(workspaceId, documentId));
  expect(res.ok()).toBe(true);
  return res.json();
}

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
}

async function documentIdFromOpenWikiPage(page: Page, wsId: string): Promise<string> {
  const match = /\/WIKI-(\d+)$/.exec(new URL(page.url()).pathname);
  expect(match).toBeTruthy();
  const treeRes = await page.request.get(`/api/v1/workspaces/${wsId}/tree`);
  expect(treeRes.ok()).toBe(true);
  const document = (await treeRes.json()).items.find(
    (item: { number: number }) => item.number === Number(match![1]),
  );
  expect(document).toBeTruthy();
  return document.id;
}

async function documentIdByTitle(page: Page, wsId: string, title: string): Promise<string> {
  const treeRes = await page.request.get(`/api/v1/workspaces/${wsId}/tree`);
  expect(treeRes.ok()).toBe(true);
  const document = (await treeRes.json()).items.find((item: { title: string }) => item.title === title);
  expect(document).toBeTruthy();
  return document.id;
}

test("member creates wiki document and edits title metadata", async ({ page }) => {
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: "acme",
    membershipRole: "member",
  });

  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.getByText("현재 역할: 멤버")).toBeVisible();

  await page.getByRole("button", { name: "새 문서" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/WIKI-\d+$/);
  await expect(page.getByLabel("문서 제목")).toHaveValue("제목 없음");

  const wsId = await workspaceId(page, "acme");
  const docId = await documentIdFromOpenWikiPage(page, wsId);
  const renamed = "연구 노트";
  const expected = { title: renamed };
  let releaseHold!: () => void;
  const held = new Promise<void>((resolve) => {
    releaseHold = resolve;
  });
  const matchUrl = (url: URL) => url.pathname === documentResourcePath(wsId, docId);
  const holdPatch = async (route: Route) => {
    const request = route.request();
    if (request.method() === "PATCH" && patchBodyHas(request, expected)) {
      await held;
    }
    await route.continue();
  };
  await page.route(matchUrl, holdPatch);

  await page.getByLabel("문서 제목").fill(renamed);
  await page.getByLabel("문서 제목").blur();
  await expect(page.getByLabel("문서 제목")).toHaveValue(renamed);
  expect((await persistedDocument(page, wsId, docId)).title).toBe("제목 없음");

  const saved = page.waitForResponse((response) =>
    isMatchingDocumentPatch(response, wsId, docId, expected),
  );
  releaseHold();
  const patch = await saved;
  expect((await patch.json()).title).toBe(renamed);
  await page.unroute(matchUrl, holdPatch);

  expect((await persistedDocument(page, wsId, docId)).title).toBe(renamed);

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: renamed })).toBeVisible();
});

test("document metadata supports icon set/clear and status changes", async ({ page }) => {
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await page.getByRole("link", { name: "연구 노트" }).click();

  const wsId = await workspaceId(page, "acme");
  const docId = await documentIdByTitle(page, wsId, "연구 노트");

  const iconSaved = page.waitForResponse((response) =>
    isMatchingDocumentPatch(response, wsId, docId, { icon: "📚" }),
  );
  await page.getByLabel("아이콘").fill("📚");
  await page.getByLabel("아이콘").blur();
  await expect(page.getByLabel("아이콘")).toHaveValue("📚");
  expect((await (await iconSaved).json()).icon).toBe("📚");
  expect((await persistedDocument(page, wsId, docId)).icon).toBe("📚");

  const statusSaved = page.waitForResponse((response) =>
    isMatchingDocumentPatch(response, wsId, docId, { status: "published" }),
  );
  await page.getByLabel("문서 상태").selectOption("published");
  await expect(page.getByLabel("문서 상태")).toHaveValue("published");
  expect((await (await statusSaved).json()).status).toBe("published");
  expect((await persistedDocument(page, wsId, docId)).status).toBe("published");

  const iconCleared = page.waitForResponse((response) =>
    isMatchingDocumentPatch(response, wsId, docId, { icon: null }),
  );
  await page.getByLabel("아이콘").fill("");
  await page.getByLabel("아이콘").blur();
  await expect(page.getByLabel("아이콘")).toHaveValue("");
  expect((await (await iconCleared).json()).icon).toBeNull();
  const persisted = await persistedDocument(page, wsId, docId);
  expect(persisted.title).toBe("연구 노트");
  expect(persisted.icon).toBeNull();
  expect(persisted.status).toBe("published");

  await page.reload();
  await expect(page.getByLabel("문서 제목")).toHaveValue("연구 노트");
  await expect(page.getByLabel("아이콘")).toHaveValue("");
  await expect(page.getByLabel("문서 상태")).toHaveValue("published");
});

test("document metadata save failure keeps the previous status", async ({ page }) => {
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await page.getByRole("link", { name: "연구 노트" }).click();
  await expect(page.getByLabel("문서 상태")).toHaveValue("published");

  const wsId = await workspaceId(page, "acme");
  const docId = await documentIdByTitle(page, wsId, "연구 노트");

  await page.route("**/api/v1/workspaces/*/documents/*", async (route) => {
    if (route.request().method() === "PATCH") {
      await route.abort("connectionfailed");
      return;
    }
    await route.continue();
  });

  await page.getByLabel("문서 상태").selectOption("archived");
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.getByLabel("문서 상태")).toHaveValue("published");
  expect((await persistedDocument(page, wsId, docId)).status).toBe("published");
});

test("member create transport failure shows an error without navigation", async ({ page }) => {
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await page.route("**/api/v1/workspaces/*/documents", async (route) => {
    if (route.request().method() === "POST") {
      await route.abort("connectionfailed");
      return;
    }
    await route.continue();
  });
  await page.getByRole("button", { name: "새 문서" }).click();
  await expect(page.getByRole("alert")).toContainText("연결을 확인하고 다시 시도해 주세요");
  await expect(page).toHaveURL(/\/w\/acme\/wiki$/);
});

test("nested wiki tree renders more than two levels", async ({ page }) => {
  await login(page, admin.email, admin.password);
  const id = await workspaceId(page, "acme");

  const rootRes = await page.request.post(`/api/v1/workspaces/${id}/documents`, {
    data: { parentId: null, title: "루트 문서" },
  });
  expect(rootRes.ok()).toBe(true);
  const root = await rootRes.json();

  const childRes = await page.request.post(`/api/v1/workspaces/${id}/documents`, {
    data: { parentId: root.id, title: "중간 문서" },
  });
  expect(childRes.ok()).toBe(true);
  const child = await childRes.json();

  const grandchildRes = await page.request.post(`/api/v1/workspaces/${id}/documents`, {
    data: { parentId: child.id, title: "하위 문서" },
  });
  expect(grandchildRes.ok()).toBe(true);

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: "루트 문서" })).toBeVisible();
  await expect(page.getByRole("link", { name: "중간 문서" })).toBeVisible();
  await expect(page.getByRole("link", { name: "하위 문서" })).toBeVisible();
});

test("admin sees member document and guest cannot read wiki", async ({ page }) => {
  await login(page, admin.email, admin.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: "연구 노트" })).toBeVisible();

  const id = await workspaceId(page, "acme");
  const treeRes = await page.request.get(`/api/v1/workspaces/${id}/tree`);
  expect(treeRes.ok()).toBe(true);
  const treeBody = await treeRes.json();
  const document = treeBody.items.find((item: { title: string }) => item.title === "연구 노트");
  expect(document).toBeTruthy();

  createE2eUser(guest.email, guest.password, guest.givenName, {
    familyName: guest.familyName,
    workspaceSlug: "acme",
    membershipRole: "guest",
  });

  await logout(page);
  await login(page, guest.email, guest.password);

  const guestTreeRes = await page.request.get(`/api/v1/workspaces/${id}/tree`);
  expect(guestTreeRes.ok()).toBe(true);
  expect((await guestTreeRes.json()).items).toEqual([]);

  const guestDocRes = await page.request.get(
    `/api/v1/workspaces/${id}/documents/${document.id}`,
  );
  expect(guestDocRes.status()).toBe(404);
  expect((await guestDocRes.json()).code).toBe("not_found");

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.getByText("현재 역할: 게스트")).toBeVisible();
  await expect(page.getByRole("button", { name: "새 문서" })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "연구 노트" })).toHaveCount(0);
  await expect(page.getByText("문서가 없습니다")).toBeVisible();

  await page.goto(`/w/acme/WIKI-${document.number}`);
  await expect(page).toHaveURL(/\/w\/acme\/wiki$/);
});
