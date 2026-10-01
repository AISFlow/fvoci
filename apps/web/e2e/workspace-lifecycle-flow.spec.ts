import { expect, test, type Page } from "@playwright/test";
import { readJson, flowSchemas, createE2eUser, login, logout } from "./helpers";

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "lifecycle-member@example.com",
  password: "memberpass1",
  givenName: "멤버",
  familyName: "이",
};

test("home counts and owner deletes a team workspace", async ({ page }) => {
  test.setTimeout(90000);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15000 });
  await page.getByLabel("성").fill(admin.familyName);
  await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
  await page.getByLabel("이메일").fill(admin.email);
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
  await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("heading", { name: "나의 대시보드" })).toBeVisible();
  await expect(page.getByText("문서 0개")).toBeVisible();
  await expect(page.getByText("담당 태스크 0개")).toBeVisible();

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await readJson(workspacesRes, flowSchemas.workspaces);
  const acme = workspacesBody.items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  if (acme === undefined) throw new Error("Missing fixture value: acme");
  expect(acme).toBeTruthy();
  const created = await page.request.post(`/api/v1/workspaces/${acme.id}/documents`, {
    data: { title: "수명주기 문서", parentId: null },
  });
  expect(created.ok()).toBe(true);

  await page.reload();
  await expect(page.getByText("문서 1개")).toBeVisible();

  await page.getByRole("button", { name: "만들기" }).click();
  await page.getByLabel("워크스페이스 이름").fill("Beta 팀");
  await page.getByLabel("주소(영문)").fill("beta-team");
  await page.getByRole("dialog").getByRole("button", { name: "만들기" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Beta 팀" })).toBeVisible();

  await page.getByRole("link", { name: "Beta 팀" }).click();
  await expect(page).toHaveURL(/\/w\/beta-team\/wiki$/);
  await page.goto("/w/beta-team/settings");
  await page
    .locator("summary")
    .filter({ hasText: /^워크스페이스 삭제$/ })
    .click();
  await page.getByLabel("확인을 위해 주소(영문)를 입력하세요.").fill("beta-team");
  await page.getByRole("button", { name: "워크스페이스 삭제" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("link", { name: "Beta 팀" })).toHaveCount(0);
  await expect(page.getByText(admin.workspaceName)).toBeVisible();

  await logout(page);
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });
  await login(page, member.email, member.password);
  await page.goto("/w/acme/settings");
  await expect(page.getByText("설정을 변경하려면 관리자 권한이 필요합니다")).toBeVisible();
  await expect(page.locator("summary").filter({ hasText: /^워크스페이스 삭제$/ })).toHaveCount(0);
});

function barrier() {
  let complete!: () => void;
  const promise = new Promise<void>((resolve) => {
    complete = resolve;
  });
  return { promise, release: complete };
}

async function ownedWorkspace(page: Page, slug: string) {
  await login(page, admin.email, admin.password);
  const response = await page.request.post("/api/v1/workspaces", { data: { name: slug, slug } });
  expect(response.status()).toBe(201);
  const list = await readJson(
    await page.request.get("/api/v1/me/workspaces"),
    flowSchemas.workspaces,
  );
  const workspace = list.items.find((item) => item.slug === slug);
  const acme = list.items.find((item) => item.slug === admin.workspaceSlug);
  if (!workspace || !acme) throw new Error("missing isolated workspace fixture");
  return { workspace, acme };
}

async function confirmDelete(page: Page, slug: string) {
  await page
    .locator("summary")
    .filter({ hasText: /^워크스페이스 삭제$/ })
    .click();
  await page.getByLabel("확인을 위해 주소(영문)를 입력하세요.").fill(slug);
  await page.getByRole("button", { name: "워크스페이스 삭제" }).click();
}

test("confirmed delete keeps clean home when old metadata404 settles before its document commits", async ({
  page,
}) => {
  const slug = "delete-order";
  const { workspace } = await ownedWorkspace(page, slug);
  const metadata = barrier();
  const metadataStarted = barrier();
  const metadataDelivered = barrier();
  const home = barrier();
  const homeStarted = barrier();
  const destinations: string[] = [];
  await page.route(`**/api/v1/workspaces/${workspace.id}`, async (route) => {
    if (route.request().method() !== "GET") return route.continue();
    metadataStarted.release();
    await metadata.promise;
    const response = await route.fetch();
    expect(response.status()).toBe(404); // Real deleted workspace, not a fabricated query error.
    await route.fulfill({ response });
    metadataDelivered.release();
  });
  await page.route(
    (url) => url.pathname === "/",
    async (route) => {
      if (!route.request().isNavigationRequest()) return route.continue();
      const url = new URL(route.request().url());
      destinations.push(url.pathname + url.search);
      homeStarted.release();
      await home.promise;
      await route.continue();
    },
  );
  try {
    await page.goto(`/w/${slug}/settings`);
    await metadataStarted.promise;
    const ack = page.waitForResponse(
      (response) =>
        response.request().method() === "DELETE" &&
        new URL(response.url()).pathname === `/api/v1/workspaces/${workspace.id}`,
    );
    await confirmDelete(page, slug);
    expect((await ack).status()).toBe(200);
    await homeStarted.promise;
    metadata.release();
    await metadataDelivered.promise;
    // Do not evaluate the departing realm: its document navigation is held.
    // The server404 has been delivered; now release the document and assert
    // the final destination plus every competing navigation attempt.
    home.release();
    await expect(page).toHaveURL(/\/$/);
    expect(destinations).toEqual(["/"]);
    await expect(page.getByRole("link", { name: slug, exact: true })).toHaveCount(0);
    await expect(page.getByText(admin.workspaceName)).toBeVisible();
  } finally {
    metadata.release();
    home.release();
  }
});

for (const outcome of ["success", "failure"] as const) {
  test(`late delete ${outcome} cannot navigate from or invalidate another workspace`, async ({
    page,
  }) => {
    const slug = `delete-switch-${outcome}`;
    const { workspace, acme } = await ownedWorkspace(page, slug);
    const requestStarted = barrier();
    const release = barrier();
    const delivered = barrier();
    await page.route(`**/api/v1/workspaces/${workspace.id}`, async (route) => {
      if (route.request().method() !== "DELETE") return route.continue();
      requestStarted.release();
      await release.promise;
      if (outcome === "success") {
        const response = await route.fetch();
        expect(response.status()).toBe(200);
        await route.fulfill({ response });
      } else {
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ title: "controlled delete unavailable", status: 503 }),
        });
      }
      delivered.release();
    });
    try {
      await page.goto(`/w/${slug}/settings`);
      await page.evaluate(() => {
        Reflect.set(window, "deleteVisit", "same document");
      });
      await confirmDelete(page, slug);
      await requestStarted.promise;
      await page.locator("#workspace-switch").selectOption(acme.id);
      await expect(page).toHaveURL(/\/w\/acme\/settings$/);
      await expect(page.getByLabel("워크스페이스 이름")).toHaveValue(admin.workspaceName);
      release.release();
      await delivered.promise;
      await page.evaluate(
        () =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() =>
              requestAnimationFrame(() => {
                resolve();
              }),
            ),
          ),
      );
      await expect(page).toHaveURL(/\/w\/acme\/settings$/);
      expect(await page.evaluate(() => Reflect.get(window, "deleteVisit") as unknown)).toBe(
        "same document",
      );
      await expect(page.getByText("controlled delete unavailable")).toHaveCount(0);
      expect((await page.request.get(`/api/v1/workspaces/${workspace.id}`)).status()).toBe(
        outcome === "success" ? 404 : 200,
      );
      expect((await page.request.get(`/api/v1/workspaces/${acme.id}`)).status()).toBe(200);
      // Success removes A; failure preserves it in the still-mounted B shell.
      await expect(page.locator(`#workspace-switch option[value="${workspace.id}"]`)).toHaveCount(
        outcome === "success" ? 0 : 1,
      );
      await page.reload();
      await expect(page.getByLabel("워크스페이스 이름")).toHaveValue(admin.workspaceName);
    } finally {
      release.release();
    }
  });
}

test("failed delete stays in settings and a later external workspace loss still denies reentry", async ({
  page,
}) => {
  const slug = "delete-failure";
  const { workspace } = await ownedWorkspace(page, slug);
  await page.goto(`/w/${slug}/settings`);
  await page.route(`**/api/v1/workspaces/${workspace.id}`, async (route) => {
    if (route.request().method() !== "DELETE") return route.continue();
    await route.fulfill({
      status: 503,
      contentType: "application/problem+json",
      body: JSON.stringify({ title: "controlled delete unavailable", status: 503 }),
    });
  });
  await confirmDelete(page, slug);
  await expect(page.getByText("controlled delete unavailable")).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/w/${slug}/settings$`));
  expect((await page.request.get(`/api/v1/workspaces/${workspace.id}`)).status()).toBe(200);
  const removed = await page.request.delete(`/api/v1/workspaces/${workspace.id}`, {
    data: { confirmSlug: slug },
  });
  expect(removed.status()).toBe(200); // Request context bypasses the browser-only fault route.
  await expect(page).toHaveURL(/\/\?denied=workspace$/);
  expect((await page.request.get(`/api/v1/workspaces/${workspace.id}`)).status()).toBe(404);
  await page.goto(`/w/${slug}/settings`);
  await expect(page).toHaveURL(/\/\?denied=workspace$/);
  const list = await readJson(
    await page.request.get("/api/v1/me/workspaces"),
    flowSchemas.workspaces,
  );
  expect(list.items.some((item) => item.slug === admin.workspaceSlug)).toBe(true);
});
