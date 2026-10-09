import { expect, test } from "@playwright/test";
import { readJson, flowSchemas, login } from "./helpers";

test.describe.configure({ mode: "serial" });
const owner = { email: "fallback@example.com", password: "fallbackpass123" };
let workspaceId: string;

test("generic workspace refs keep the real setup gate before canonicalization", async ({
  page,
}) => {
  await page.goto("/w/fallback/%20OPS%20?from=setup#overview");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("경계");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Fallback workspace");
  await page.getByLabel("주소(영문)").fill("fallback");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  const fixtureValue1 = (
    await readJson(await page.request.get("/api/v1/me/workspaces"), flowSchemas.workspaces)
  ).items[0];
  if (fixtureValue1 === undefined)
    throw new Error(
      'Missing fixture value: (\n    await readJson(await page.request.get("/api/v1/me/workspaces"), flowSchemas.workspaces)\n  ).items[0]',
    );
  workspaceId = fixtureValue1.id;
  const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key: "OPS", name: "Canonical project", visibility: "workspace" },
  });
  expect(response.status()).toBe(201);
  const project = await readJson(response, flowSchemas.project);
  expect(
    (
      await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`, {
        data: { title: "Canonical task", startDate: "2026-09-28", dueDate: "2026-09-30" },
      })
    ).status(),
  ).toBe(201);
  expect(
    (
      await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, {
        data: { commandId: crypto.randomUUID(), title: "Canonical wiki", parentId: null },
      })
    ).status(),
  ).toBe(201);
});

test("encoded, trimmed and NFKC project refs canonicalize once inside Vue with query and fragment", async ({
  page,
}) => {
  await login(page, owner.email, owner.password);
  await page.addInitScript(() => {
    if (window.top === window) {
      sessionStorage.setItem(
        "fallbackBoots",
        String(Number(sessionStorage.getItem("fallbackBoots") ?? 0) + 1),
      );
    }
  });
  for (const ref of ["%4FPS", "%20ops%20", "%EF%BC%AF%EF%BC%B0%EF%BC%B3"]) {
    const before = await page.evaluate(() => Number(sessionStorage.getItem("fallbackBoots") ?? 0));
    await page.goto(`/w/fallback/${ref}?from=encoded%20ref#overview`);
    await expect(page).toHaveURL(/\/OPS\?from=encoded%20ref#overview$/);
    await expect(
      page.getByRole("heading", { name: "Canonical project", exact: true }),
    ).toBeVisible();
    await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
    expect(await page.evaluate(() => Number(sessionStorage.getItem("fallbackBoots")))).toBe(
      before + 1,
    );
  }
  for (const [ref, title, target] of [
    ["%4FPS-2", "Canonical task", "OPS-2"],
    ["%57IKI-1", "Canonical wiki", "WIKI-1"],
  ] as const) {
    const before = await page.evaluate(() => Number(sessionStorage.getItem("fallbackBoots")));
    await page.goto(`/w/fallback/${ref}?from=item#document-comments`);
    await expect(page).toHaveURL(new RegExp(`/${target}\\?from=item#document-comments$`));
    const required1 = title;
    if (target === "WIKI-1") {
      await expect(page.getByRole("textbox", { name: "문서 제목", exact: true })).toHaveValue(
        required1,
      );
    } else {
      await expect(page.getByRole("heading", { name: title, exact: true })).toBeVisible();
    }
    expect(await page.evaluate(() => Number(sessionStorage.getItem("fallbackBoots")))).toBe(
      before + 1,
    );
  }
});

test("invalid authorized refs retain the workspace shell; unknown nested paths go home without a reload loop", async ({
  page,
}) => {
  await login(page, owner.email, owner.password);
  for (const ref of ["WIKI-01", "bad!", "a"]) {
    await page.goto(`/w/fallback/${ref}?from=invalid#anchor`);
    await expect(page.getByRole("alert")).toHaveText("요청한 항목을 찾을 수 없습니다");
    await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
    expect(new URL(page.url()).pathname).toBe(`/w/fallback/${ref}`);
  }
  for (const path of ["/w/fallback/wiki/extra", "/settings/account/extra", "/unknown/nested"]) {
    await page.goto(path);
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByRole("link", { name: /Fallback workspace/ })).toBeVisible();
    await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  }
});

test("fallback refs enforce real missing-session and inaccessible-workspace gates", async ({
  page,
  browser,
}) => {
  await login(page, owner.email, owner.password);
  await page.goto("/w/absent/%20OPS%20?from=denied#overview");
  await expect(page).toHaveURL(/\/\?denied=workspace$/);
  const context = await browser.newContext();
  try {
    const signedOut = await context.newPage();
    await signedOut.goto(
      `${new URL(page.url()).origin}/w/fallback/%20OPS%20?from=signed-out#overview`,
    );
    await expect(signedOut).toHaveURL(/\/login\?returnTo=/);
    expect(new URL(signedOut.url()).searchParams.get("returnTo")).toBe(
      "/w/fallback/%20OPS%20?from=signed-out#overview",
    );
    await expect(signedOut.getByLabel("이메일")).toBeVisible();
  } finally {
    await context.close();
  }
});

test("cold home, legal and populated Gantt record their actual production asset requests", async ({
  page,
  browser,
}, testInfo) => {
  await login(page, owner.email, owner.password);
  const origin = new URL(page.url()).origin;
  const storageState = await page.context().storageState();
  const witnesses = [];
  for (const path of ["/", "/legal/privacy", "/w/fallback/OPS/gantt?y=2026&m=9"]) {
    const context = await browser.newContext({ storageState });
    try {
      const cold = await context.newPage();
      const requests: string[] = [];
      const responses: {
        url: string;
        status: number;
        contentType: string;
      }[] = [];
      cold.on("request", (request) => {
        requests.push(request.url());
      });
      cold.on("response", (response) => {
        responses.push({
          url: response.url(),
          status: response.status(),
          contentType: response.headers()["content-type"] ?? "",
        });
      });
      await cold.goto(origin + path);
      await expect(cold.locator("#root[data-v-app]")).toHaveCount(1);
      if (new URL(origin + path).pathname.endsWith("gantt")) {
        await expect(cold.getByRole("searchbox", { name: "태스크 검색" })).toBeVisible();
        await expect(
          cold
            .getByRole("region", { name: "간트", exact: true })
            .getByText("Canonical task", { exact: true }),
        ).toBeVisible();
      } else {
        await expect(cold.locator("main")).toBeVisible();
      }
      await cold.evaluate(
        () =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() =>
              requestAnimationFrame(() => {
                resolve();
              }),
            ),
          ),
      );
      witnesses.push({
        path,
        requests,
        responses,
        resources: await cold.evaluate(() =>
          performance.getEntriesByType("resource").map((entry) => ({
            name: entry.name,
            initiatorType: (entry as PerformanceResourceTiming).initiatorType,
          })),
        ),
      });
    } finally {
      await context.close();
    }
  }
  await testInfo.attach("cold-production-network", {
    body: JSON.stringify(witnesses, null, 2),
    contentType: "application/json",
  });
});

test("cold wiki loads its editor and persists a real body before reload", async ({
  page,
}, testInfo) => {
  await login(page, owner.email, owner.password);
  const requests: string[] = [];
  page.on("request", (request) => {
    requests.push(request.url());
  });
  await page.goto("/w/fallback/WIKI-1");
  await expect(page.getByRole("textbox", { name: "문서 제목", exact: true })).toHaveValue(
    "Canonical wiki",
  );
  const body = page.locator(".tiptap");
  await expect(body).toBeVisible();
  await body.click();
  await body.press("ControlOrMeta+End");
  await body.pressSequentially("Fallback saved body");
  await expect(body).toContainText("Fallback saved body");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.getByRole("button", { name: "저장", exact: true })).toBeEnabled();
  await page.reload();
  await expect(body).toContainText("Fallback saved body");
  await testInfo.attach("positive-editor-network", {
    body: JSON.stringify(requests, null, 2),
    contentType: "application/json",
  });
});
