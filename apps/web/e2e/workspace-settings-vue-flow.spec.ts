import path from "node:path";
import { expect, type Page, request, test } from "@playwright/test";
import { authSql } from "./auth-link-evidence";
import { readJson, flowSchemas, createE2eUser, login, logout, watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial" });

const owner = { email: "workspace-settings-owner@example.com", password: "settingspass1" };
let workspaceId: string;

async function openSettings(page: Page, suffix = ""): Promise<void> {
  await page.goto(`/w/settings-vue/settings${suffix}`);
  await expect(page.locator("#root[data-v-app]")).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/w/settings-vue/settings${suffix}$`));
}

test("Vue settings commit identity, groups, tokens, holidays, preferences and recovered import/export", async ({
  page,
}) => {
  test.setTimeout(120000);
  const csp = watchCspViolations(page);
  const pageErrors: string[] = [];
  page.on("pageerror", (err) => pageErrors.push(err.message));
  await page.goto("/setup");
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("설정");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Settings Vue");
  await page.getByLabel("주소(영문)").fill("settings-vue");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  const workspaces = await page.request.get("/api/v1/me/workspaces");
  const fixtureValue1 = (await readJson(workspaces, flowSchemas.workspaces)).items.find(
    (item: { slug: string }) => item.slug === "settings-vue",
  );
  if (fixtureValue1 === undefined)
    throw new Error(
      'Missing fixture value: (await readJson(workspaces, flowSchemas.workspaces)).items.find(\n    (item: { slug: string }) => item.slug === "settings-vue",\n  )',
    );
  workspaceId = fixtureValue1.id;
  const metadataUrl = `**/api/v1/workspaces/${workspaceId}`;
  await page.route(metadataUrl, (route) => route.abort("failed"));
  await openSettings(page);
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(/\/settings$/);
  await page.unroute(metadataUrl);
  await page.getByRole("button", { name: "다시 시도" }).click();
  await page.getByLabel("워크스페이스 이름").fill("Settings renamed");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "저장" })).toBeVisible();
  expect(
    (
      await readJson(
        await page.request.get(`/api/v1/workspaces/${workspaceId}`),
        flowSchemas.workspace,
      )
    ).name,
  ).toBe("Settings renamed");
  const groups = page
    .locator("details")
    .filter({ has: page.locator("summary", { hasText: /^그룹$/ }) });
  await groups.locator("summary").click();
  await groups.getByLabel("그룹 이름").fill("Settings group");
  await groups.getByRole("button", { name: "만들기", exact: true }).click();
  await expect(groups.getByRole("button", { name: "Settings group", exact: true })).toBeVisible();
  await groups.getByLabel("그룹 이름").fill("x".repeat(101));
  await groups.getByRole("button", { name: "만들기", exact: true }).click();
  await expect(groups.getByRole("alert")).toBeVisible();
  await groups.getByLabel("그룹 이름").fill("Recovered group");
  await groups.getByRole("button", { name: "만들기", exact: true }).click();
  await expect(groups.getByRole("button", { name: "Recovered group", exact: true })).toBeVisible();

  const tokens = page
    .locator("details")
    .filter({ has: page.locator("summary", { hasText: /^토큰$/ }) });
  await tokens.locator("summary").click();
  await tokens.getByRole("textbox", { name: "이름", exact: true }).fill("Read-only settings token");
  await tokens.getByLabel("문서 조회").check();
  await tokens.getByRole("button", { name: "발급" }).click();
  const secretField = tokens.getByRole("textbox", { name: "이 토큰 값은 지금만 보입니다" });
  await expect(secretField).toBeVisible();
  const secret = await secretField.inputValue();
  const auth = { Authorization: `Bearer ${secret}` };
  const tokenClient = await request.newContext({
    baseURL: new URL(page.url()).origin,
    extraHTTPHeaders: auth,
  });
  expect((await tokenClient.get(`/api/v1/workspaces/${workspaceId}/tree`)).ok()).toBe(true);
  expect(
    (
      await tokenClient.post(`/api/v1/workspaces/${workspaceId}/documents`, {
        data: { title: "Denied", parentId: null },
      })
    ).status(),
  ).toBe(404);
  const listedTokens = await readJson(
    await page.request.get(`/api/v1/workspaces/${workspaceId}/api-tokens`),
    flowSchemas.unknown,
  );
  expect(JSON.stringify(listedTokens)).not.toContain(secret);
  await page.reload();
  await tokens.locator("summary").click();
  await expect(secretField).toHaveCount(0);
  await tokens.getByRole("button", { name: "폐기" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "폐기" }).click();
  await expect(tokens.getByText("토큰이 없습니다")).toBeVisible();
  expect((await tokenClient.get(`/api/v1/workspaces/${workspaceId}/tree`)).status()).toBe(401);
  await tokenClient.dispose();

  const calendar = page
    .locator("details")
    .filter({ has: page.locator("summary", { hasText: /달력 구독/ }) });
  await calendar.locator("summary").click();
  await calendar.getByLabel("공휴일 날짜").fill("2026-10-09");
  await calendar.getByRole("button", { name: "공휴일 추가" }).click();
  await expect(calendar.locator("time")).toHaveText("2026-10-09");
  await calendar.getByRole("button", { name: "2026-10-09 삭제" }).click();
  await expect(calendar.locator("time")).toHaveCount(0);
  const prefsUrl = `**/api/v1/workspaces/${workspaceId}/notification-prefs`;
  const initialPrefs = await readJson(
    await page.request.get(`/api/v1/workspaces/${workspaceId}/notification-prefs`),
    flowSchemas.prefs,
  );
  await page.route(prefsUrl, (route) =>
    route.request().method() === "PUT" ? route.abort("failed") : route.continue(),
  );
  const prefsSection = page
    .locator("section")
    .filter({ has: page.getByRole("heading", { name: "알림", exact: true }) });
  await page.getByLabel("인앱 알림", { exact: true }).click();
  await expect(prefsSection.getByRole("alert")).toBeVisible();
  await expect(page.getByLabel("인앱 알림", { exact: true })).toBeChecked({
    checked: initialPrefs.inApp,
  });
  await page.unroute(prefsUrl);
  await page.getByLabel("메일 다이제스트").click();
  await expect
    .poll(
      async () =>
        (
          await readJson(
            await page.request.get(`/api/v1/workspaces/${workspaceId}/notification-prefs`),
            flowSchemas.prefs,
          )
        ).mailDigest,
    )
    .toBe(!initialPrefs.mailDigest);

  // Only the failed request is intercepted. Recovery downloads actual Rust output.
  const exportUrl = `**/api/v1/workspaces/${workspaceId}/export`;
  await page.route(exportUrl, (route) => route.abort("failed"));
  const exportSection = page
    .locator("section")
    .filter({ has: page.getByRole("heading", { name: "워크스페이스 내보내기", exact: true }) });
  await exportSection.getByRole("button").click();
  await expect(exportSection.getByRole("alert")).toBeVisible();
  await page.unroute(exportUrl);
  const downloadPromise = page.waitForEvent("download");
  await exportSection.getByRole("button").click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("fvoci-workspace.zip");
  await expect(exportSection.getByRole("alert")).toHaveCount(0);
  await page.locator('input[type="file"]').setInputFiles({
    name: "invalid.zip",
    mimeType: "application/zip",
    buffer: Buffer.from("not a zip"),
  });
  const importSection = page
    .locator("section")
    .filter({ has: page.getByRole("heading", { name: "가져올 형식", exact: true }) });
  await expect(importSection.getByRole("alert")).toBeVisible();
  await expect(importSection.getByRole("status")).toHaveCount(0);
  await page
    .locator('input[type="file"]')
    .setInputFiles(path.resolve(import.meta.dirname, "fixtures/markdown-import.zip"));
  await expect(importSection.getByRole("status")).toHaveText("가져오기를 시작했습니다", {
    timeout: 30000,
  });
  const documents = await readJson(
    await page.request.get(`/api/v1/workspaces/${workspaceId}/tree`),
    flowSchemas.documents,
  );
  expect(documents.items.some((item: { title: string }) => item.title === "e2e-note")).toBe(true);
  // An interrupted status read resumes the existing durable job, without reuploading.
  const statusUrl = `**/api/v1/import/*?workspaceId=${workspaceId}`;
  await page.route(statusUrl, (route) => route.abort("failed"));
  await page.getByLabel("가져올 형식").selectOption("office-file");
  await page.locator('input[type="file"]').setInputFiles({
    name: "resume-settings.md",
    mimeType: "text/markdown",
    buffer: Buffer.from("# Resume settings\n\nDurable status recovery"),
  });
  await expect(importSection.getByRole("button", { name: "상태 다시 확인" })).toBeVisible();
  await page.unroute(statusUrl);
  await importSection.getByRole("button", { name: "상태 다시 확인" }).click();
  await expect(importSection.getByRole("status")).toHaveText("가져오기를 시작했습니다", {
    timeout: 30000,
  });
  const afterResume = await readJson(
    await page.request.get(`/api/v1/workspaces/${workspaceId}/tree`),
    flowSchemas.documents,
  );
  expect(
    afterResume.items.filter((item: { title: string }) => item.title === "resume-settings"),
  ).toHaveLength(1);

  const sso = page
    .locator("details")
    .filter({ has: page.locator("summary", { hasText: /^싱글 사인온$/ }) });
  await sso.locator("summary").click();
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/oidc`)).status()).toBe(404);
  await expect(sso.locator("form")).toHaveCount(0);
  await expect(page.getByTestId("workspace-events")).toContainText("workspace");
  expect(csp).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("actual Vue tags and templates URLs persist edits, apply documents, and enforce member restrictions", async ({
  page,
}) => {
  test.setTimeout(90000);
  const csp = watchCspViolations(page);
  await login(page, owner.email, owner.password);
  await openSettings(page, "/document-tags");
  await page.getByLabel("이름", { exact: true }).fill("Settings tag");
  await page.getByLabel("색", { exact: true }).selectOption("blue");
  await page.getByRole("button", { name: "만들기", exact: true }).click();
  const row = page.getByTestId("document-tag-row-Settings tag");
  await expect(row).toBeVisible();
  await row.getByRole("textbox").fill("Renamed settings tag");
  await row.getByRole("textbox").press("Enter");
  await expect(page.getByTestId("document-tag-row-Renamed settings tag")).toBeVisible();
  await page.reload();
  await expect(page.getByTestId("document-tag-row-Renamed settings tag")).toBeVisible();
  await openSettings(page, "/templates");
  await page.getByLabel("제목").fill("Settings document template");
  await page.getByRole("button", { name: "추가", exact: true }).click();
  const template = page.getByRole("row", { name: /Settings document template/ });
  await expect(template).toBeVisible();
  await template.getByRole("button", { name: "적용" }).click();
  await expect(page).toHaveURL(/\/w\/settings-vue\/WIKI-\d+$/);
  await expect(page.getByLabel("문서 제목")).toHaveValue("Settings document template");

  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key: "REC", name: "Recovered settings project", visibility: "workspace" },
  });
  expect(projectResponse.status()).toBe(201);
  const project = await readJson(projectResponse, flowSchemas.project);
  expect(
    (await page.request.delete(`/api/v1/workspaces/${workspaceId}/projects/${project.id}`)).ok(),
  ).toBe(true);
  await openSettings(page);
  await page
    .getByTestId("deleted-projects")
    .getByRole("button", { name: "복원 Recovered settings project" })
    .click();
  await page.getByRole("dialog").getByRole("button", { name: "복원", exact: true }).click();
  await expect(page.getByTestId("deleted-projects")).toHaveCount(0);
  const activeProjects = await readJson(
    await page.request.get(`/api/v1/workspaces/${workspaceId}/projects`),
    flowSchemas.projects,
  );
  expect(activeProjects.items.some((item: { id: string }) => item.id === project.id)).toBe(true);

  await logout(page);
  createE2eUser("settings-member@example.com", "memberpass1", "멤버", {
    familyName: "이",
    workspaceSlug: "settings-vue",
    membershipRole: "member",
  });
  await login(page, "settings-member@example.com", "memberpass1");
  await openSettings(page);
  await expect(page.getByText("설정을 변경하려면 관리자 권한이 필요합니다")).toBeVisible();
  await expect(page.getByRole("button", { name: "워크스페이스 내보내기" })).toHaveCount(0);
  await expect(
    page.locator("summary").filter({ hasText: /^(토큰|웹훅|워크스페이스 삭제)$/ }),
  ).toHaveCount(0);
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/export`)).status()).toBe(404);
  expect(
    (
      await page.request.post(`/api/v1/workspaces/${workspaceId}/groups`, {
        data: { name: "Denied group" },
      })
    ).ok(),
  ).toBe(false);
  await openSettings(page, "/document-tags");
  await expect(
    page.getByTestId("document-tag-row-Renamed settings tag").getByRole("textbox"),
  ).toHaveCount(0);
  expect(csp).toEqual([]);
});

test("workspace consents show real empty, populated, timezone, retry and role denial states", async ({
  page,
  browser,
}) => {
  await login(page, owner.email, owner.password);
  await openSettings(page);
  const section = page.getByTestId("workspace-consents");
  await section.locator("summary").click();
  await expect(section).toContainText("기록된 동의가 없습니다");
  expect(
    await readJson(
      await page.request.get(`/api/v1/workspaces/${workspaceId}/consents`),
      flowSchemas.consents,
    ),
  ).toEqual({ members: [] });

  const published = await page.request.post("/api/v1/admin/legal", {
    data: {
      kind: "terms",
      title: "Settings consent terms",
      bodyMarkdown: "Settings consent terms",
      effectiveAt: "2026-01-01T00:00:00Z",
      required: false,
    },
  });
  expect(published.status()).toBe(201);
  const version = (await readJson(published, flowSchemas.version)).version;
  expect(
    (
      await page.request.post("/api/v1/auth/consents", {
        data: { items: [{ kind: "terms", version }] },
      })
    ).ok(),
  ).toBe(true);
  const currentUserId = (
    await readJson(await page.request.get("/api/v1/auth/me"), flowSchemas.user)
  ).userId;
  // Fixture timestamp straddles a UTC day; the production read must use the
  // user's persisted timezone, rather than the browser's or server's zone.
  authSql(`UPDATE fvoci.users SET timezone = 'America/Los_Angeles' WHERE id = '${currentUserId}';
    UPDATE fvoci.user_consents SET consented_at = '2026-01-01T00:30:00Z' WHERE user_id = '${currentUserId}' AND kind = 'terms' AND version = ${String(version)}`);
  const consentUrl = `**/api/v1/workspaces/${workspaceId}/consents`;
  await page.route(consentUrl, (route) => route.abort("failed"));
  await page.reload();
  await section.locator("summary").click();
  await expect(section.getByRole("alert")).toBeVisible();
  await expect(section.getByText("기록된 동의가 없습니다")).toHaveCount(0);
  await page.unroute(consentUrl);
  await section.getByRole("button", { name: "다시 시도" }).click();
  await expect(section.getByRole("alert")).toHaveCount(0);
  const row = section
    .getByRole("row")
    .filter({ has: page.getByRole("cell", { name: "terms", exact: true }) });
  await expect(row.getByRole("cell").nth(0)).toHaveText("김설정");
  await expect(row.getByRole("cell").nth(2)).toHaveText(String(version));
  await expect(row.getByRole("cell").nth(3)).toHaveText("2025. 12. 31.");
  const stored = await readJson(
    await page.request.get(`/api/v1/workspaces/${workspaceId}/consents`),
    flowSchemas.consents,
  );
  const fixtureValue2 = stored.members.find(
    (member: { userId: string }) => member.userId === currentUserId,
  );
  if (fixtureValue2 === undefined)
    throw new Error(
      "Missing fixture value: stored.members.find((member: { userId: string }) => member.userId === currentUserId)",
    );
  expect(fixtureValue2.consents).toEqual([
    { kind: "terms", version, consentedAt: "2026-01-01T00:30:00Z" },
  ]);
  // Names are a separate query: a transient name lookup failure must keep
  // actual consent rows usable with the original user-ID fallback.
  await page.route(`**/api/v1/workspaces/${workspaceId}/members`, (route) => route.abort("failed"));
  await page.reload();
  await section.locator("summary").click();
  await expect(row.getByRole("cell").nth(0)).toHaveText(currentUserId);
  await page.unroute(`**/api/v1/workspaces/${workspaceId}/members`);
  authSql(`UPDATE fvoci.users SET timezone = 'Asia/Seoul' WHERE id = '${currentUserId}'`);

  for (const role of ["admin", "member"]) {
    const email = `settings-consent-${role}@example.com`;
    createE2eUser(email, "consentpass1", role, {
      workspaceSlug: "settings-vue",
      membershipRole: role,
    });
    const rolePage = await browser.newPage();
    try {
      await login(rolePage, email, "consentpass1");
      let consentRequests = 0;
      rolePage.on("request", (req) => {
        if (req.url().endsWith(`/workspaces/${workspaceId}/consents`)) {
          consentRequests += 1;
        }
      });
      await openSettings(rolePage);
      if (role === "admin") {
        await rolePage.getByTestId("workspace-consents").locator("summary").click();
        await expect(
          rolePage
            .getByTestId("workspace-consents")
            .getByRole("cell", { name: "terms", exact: true }),
        ).toBeVisible();
        expect(
          (await rolePage.request.get(`/api/v1/workspaces/${workspaceId}/consents`)).status(),
        ).toBe(200);
      } else {
        await expect(
          rolePage.getByText("설정을 변경하려면 관리자 권한이 필요합니다"),
        ).toBeVisible();
        await expect(rolePage.getByTestId("workspace-consents")).toHaveCount(0);
        expect(consentRequests).toBe(0);
        expect(
          (await rolePage.request.get(`/api/v1/workspaces/${workspaceId}/consents`)).status(),
        ).toBe(404);
      }
    } finally {
      await rolePage.close();
    }
  }
});

test("late workspace rename and delete success or failure cannot change the switched workspace", async ({
  page,
}) => {
  await login(page, owner.email, owner.password);
  const createdB = await page.request.post("/api/v1/workspaces", {
    data: { name: "Race workspace B", slug: "settings-race-b" },
  });
  expect(createdB.status()).toBe(201);
  const workspaceB = await readJson(createdB, flowSchemas.workspace);
  for (const operation of ["rename", "delete"] as const) {
    for (const outcome of ["success", "failure"] as const) {
      const slugA = `settings-race-${operation}-${outcome}`;
      const nameA = `Race ${operation} ${outcome}`;
      const createdA = await page.request.post("/api/v1/workspaces", {
        data: { name: nameA, slug: slugA },
      });
      expect(createdA.status()).toBe(201);
      const workspaceA = await readJson(createdA, flowSchemas.workspace);
      await page.goto(`/w/${slugA}/settings`);
      await expect(page.getByLabel("워크스페이스 이름", { exact: true })).toHaveValue(nameA);

      let release!: () => void;
      let received!: () => void;
      const responseGate = new Promise<void>((resolve) => {
        release = resolve;
      });
      const requestReceived = new Promise<void>((resolve) => {
        received = resolve;
      });
      const urlA = `**/api/v1/workspaces/${workspaceA.id}`;
      const method = operation === "rename" ? "PATCH" : "DELETE";
      await page.route(urlA, async (route) => {
        if (route.request().method() !== method) {
          return route.continue();
        }
        // Delay a real Rust request until the workspace switch. For rename failure,
        // send server-invalid input past the already-tested client validator.
        received();
        await responseGate;
        const response = await route.fetch(
          operation === "rename" && outcome === "failure"
            ? { postData: { name: "x".repeat(1001) } }
            : {},
        );
        expect(response.ok()).toBe(outcome === "success");
        await route.fulfill({ response });
      });
      try {
        if (operation === "rename") {
          await page.getByLabel("워크스페이스 이름", { exact: true }).fill(`${nameA} updated`);
          await page.getByRole("button", { name: "저장", exact: true }).click();
          await expect(page.getByRole("button", { name: "저장", exact: true })).toBeDisabled();
        } else {
          const disclosure = page
            .locator("details")
            .filter({ has: page.locator("summary", { hasText: /^워크스페이스 삭제$/ }) });
          await disclosure.locator("summary").click();
          await disclosure
            .getByRole("textbox")
            .fill(outcome === "success" ? slugA : "wrong-confirmation");
          await disclosure.getByRole("button", { name: "워크스페이스 삭제", exact: true }).click();
        }
        await requestReceived;
        // The header uses Vue routing, preserving the mutation-owning page.
        await page.getByLabel("워크스페이스 전환").selectOption(workspaceB.id);
        await expect(page).toHaveURL(/\/w\/settings-race-b\/settings$/);
        await expect(page.getByLabel("워크스페이스 이름", { exact: true })).toHaveValue(
          "Race workspace B",
        );
        await expect(page.getByRole("button", { name: "저장", exact: true })).toBeEnabled();
        release();
        // Read the real query client's state to await callbacks, including
        // invalidations, without adding a product hook or a timing sleep.
        await expect
          .poll(() =>
            page.evaluate((id) => {
              const root = document.getElementById("root") as HTMLElement & {
                __vue_app__: {
                  _context: {
                    provides: Record<
                      string,
                      {
                        getMutationCache(): {
                          getAll(): {
                            state: {
                              variables?: {
                                workspaceId?: string;
                              };
                              status: string;
                            };
                          }[];
                        };
                      }
                    >;
                  };
                };
              };
              const fixtureValue3 = root.__vue_app__._context.provides.VUE_QUERY_CLIENT;
              if (fixtureValue3 === undefined)
                throw new Error(
                  "Missing fixture value: root.__vue_app__._context.provides.VUE_QUERY_CLIENT",
                );
              const mutation = fixtureValue3
                .getMutationCache()
                .getAll()
                .find((item) => item.state.variables?.workspaceId === id);
              return mutation?.state.status;
            }, workspaceA.id),
          )
          .toBe(outcome === "success" ? "success" : "error");
        await expect(page).toHaveURL(/\/w\/settings-race-b\/settings$/);
        await expect(page.getByText("저장했습니다", { exact: true })).toHaveCount(0);
        await expect(
          page.locator(".settings-page > .settings-section").first().getByRole("alert"),
        ).toHaveCount(0);
        await expect(page.getByLabel("워크스페이스 이름", { exact: true })).toHaveValue(
          "Race workspace B",
        );
        expect(
          (
            await readJson(
              await page.request.get(`/api/v1/workspaces/${workspaceB.id}`),
              flowSchemas.workspace,
            )
          ).name,
        ).toBe("Race workspace B");
        if (outcome === "success") {
          expect(
            await page.evaluate((id) => {
              const root = document.getElementById("root") as HTMLElement & {
                __vue_app__: {
                  _context: {
                    provides: Record<
                      string,
                      {
                        getQueryState(key: string[]):
                          | {
                              isInvalidated: boolean;
                            }
                          | undefined;
                      }
                    >;
                  };
                };
              };
              const fixtureValue5 = root.__vue_app__._context.provides.VUE_QUERY_CLIENT;
              if (fixtureValue5 === undefined)
                throw new Error(
                  "Missing fixture value: root.__vue_app__._context.provides.VUE_QUERY_CLIENT",
                );
              const fixtureValue4 = fixtureValue5.getQueryState(["workspaces", id]);
              if (fixtureValue4 === undefined)
                throw new Error(
                  'Missing fixture value: root.__vue_app__._context.provides.VUE_QUERY_CLIENT.getQueryState([\n                "workspaces",\n                id,\n              ])',
                );
              return fixtureValue4.isInvalidated;
            }, workspaceA.id),
          ).toBe(true);
        }
        if (operation === "rename") {
          expect(
            (
              await readJson(
                await page.request.get(`/api/v1/workspaces/${workspaceA.id}`),
                flowSchemas.workspace,
              )
            ).name,
          ).toBe(outcome === "success" ? `${nameA} updated` : nameA);
          await page.getByLabel("워크스페이스 전환").selectOption(workspaceA.id);
          await expect(page.getByLabel("워크스페이스 이름", { exact: true })).toHaveValue(
            outcome === "success" ? `${nameA} updated` : nameA,
          );
          await expect(page.getByText("저장했습니다", { exact: true })).toHaveCount(0);
        } else {
          expect((await page.request.get(`/api/v1/workspaces/${workspaceA.id}`)).status()).toBe(
            outcome === "success" ? 404 : 200,
          );
        }
      } finally {
        release();
        await page.unroute(urlA);
      }
    }
  }
});
