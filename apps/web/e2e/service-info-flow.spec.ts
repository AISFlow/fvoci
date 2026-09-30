import { readJson, flowSchemas } from "./helpers";
import { expect, test, type Browser, type Page } from "@playwright/test";

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const fullOperator = {
  businessName: "주식회사 에2이 테스트",
  representative: "김운영",
  registrationNumber: "123-45-67890",
  mailOrderNumber: "제2026-서울-0001호",
  address: "서울특별시 중구 세종대로 110",
  phone: "02-1234-5678",
  supportEmail: "support@example.com",
  businessInfoUrl: "https://www.ftc.go.kr/bizCommPop.do?wrkr_no=1234567890",
  hostingProvider: "Amazon Web Services",
};

async function runSetup(page: Page): Promise<void> {
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
}

async function patchOperator(
  page: Page,
  operator: Record<string, string | null> | null,
): Promise<void> {
  const res = await page.request.patch("/api/v1/admin/instance-settings", {
    data: operator === null ? { operator: null } : { operator },
  });
  expect(res.status()).toBe(200);
  const publicRes = await page.request.get("/api/v1/instance");
  expect(publicRes.ok()).toBe(true);
  const body = (await readJson(publicRes, flowSchemas.instance)) as {
    values: {
      operator: Record<string, string | null>;
    };
  };
  if (operator === null) {
    expect(body.values.operator.businessName).toBeNull();
    return;
  }
  for (const [key, value] of Object.entries(operator)) {
    expect(body.values.operator[key]).toBe(value);
  }
}

function loginFooter(page: Page) {
  return page.locator(".auth-shell__footer");
}

async function withAnonymous(browser: Browser, fn: (page: Page) => Promise<void>): Promise<void> {
  const context = await browser.newContext();
  const anon = await context.newPage();
  try {
    await fn(anon);
  } finally {
    await context.close();
  }
}

test("operator settings persist through admin API and surface on public service-info, login footer, and authenticated nav", async ({
  page,
  browser,
}) => {
  test.setTimeout(120000);
  await runSetup(page);

  await withAnonymous(browser, async (anonEmpty) => {
    await anonEmpty.goto("/service-info");
    await expect(anonEmpty.getByRole("heading", { name: "서비스 정보" })).toBeVisible();
    await expect(anonEmpty.getByText("등록된 서비스 운영 정보가 없습니다.")).toBeVisible();
    await anonEmpty.goto("/login");
    const emptyFooter = loginFooter(anonEmpty);
    await expect(emptyFooter.getByRole("link", { name: "이용약관" })).toHaveAttribute(
      "href",
      "/legal/terms",
    );
    await expect(emptyFooter.getByRole("link", { name: "개인정보처리방침" })).toHaveAttribute(
      "href",
      "/legal/privacy",
    );
    await expect(emptyFooter.getByRole("link", { name: "오픈소스 고지" })).toHaveAttribute(
      "href",
      "/open-source-licenses.txt",
    );
    await expect(emptyFooter.getByRole("link", { name: "서비스 정보" })).toHaveCount(0);
  });

  await patchOperator(page, fullOperator);

  await page.goto("/settings/admin");
  const operatorSection = page.locator('section[aria-labelledby="setting-operator"]');
  await expect(operatorSection.getByLabel("operator.businessName")).toHaveValue(
    fullOperator.businessName,
  );
  await operatorSection.getByLabel("operator.supportEmail").fill("ops@example.com");
  const saved = page.waitForResponse(
    (res) =>
      res.url().endsWith("/api/v1/admin/instance-settings") && res.request().method() === "PATCH",
  );
  await operatorSection.getByRole("button", { name: "저장", exact: true }).click();
  expect((await saved).status()).toBe(200);
  const afterUi = await page.request.get("/api/v1/instance");
  expect((await readJson(afterUi, flowSchemas.instance)).values.operator.supportEmail).toBe(
    "ops@example.com",
  );
  await withAnonymous(browser, async (anonFull) => {
    await anonFull.goto("/service-info");
    await expect(anonFull.getByText("주식회사 에2이 테스트")).toBeVisible();
    await expect(anonFull.getByText("김운영")).toBeVisible();
    await expect(anonFull.getByRole("link", { name: "ops@example.com" })).toHaveAttribute(
      "href",
      "mailto:ops@example.com",
    );
    await expect(
      anonFull.getByRole("link", { name: fullOperator.businessInfoUrl }),
    ).toHaveAttribute("href", fullOperator.businessInfoUrl);
    await expect(anonFull.getByText("javascript:")).toHaveCount(0);

    await anonFull.goto("/login");
    const fullFooter = loginFooter(anonFull);
    await expect(fullFooter.getByRole("link", { name: "서비스 정보" })).toHaveAttribute(
      "href",
      "/service-info",
    );
    await expect(fullFooter.getByRole("link", { name: "이용약관" })).toBeVisible();
  });

  await page.goto("/");
  await page.locator("footer").getByRole("link", { name: "서비스 정보", exact: true }).click();
  await expect(page).toHaveURL(/\/service-info$/);
  await expect(page.getByText(fullOperator.businessName)).toBeVisible();

  await patchOperator(page, {
    businessName: "부분만 공개",
    representative: null,
    registrationNumber: null,
    mailOrderNumber: null,
    address: null,
    phone: "010-0000-0000",
    supportEmail: null,
    businessInfoUrl: null,
    hostingProvider: null,
  });

  await withAnonymous(browser, async (anonPartial) => {
    await anonPartial.goto("/service-info");
    await expect(anonPartial.getByText("부분만 공개")).toBeVisible();
    await expect(anonPartial.getByText("010-0000-0000")).toBeVisible();
    await expect(anonPartial.getByText("대표자")).toHaveCount(0);
    await expect(anonPartial.getByRole("link")).toHaveCount(0);
  });
});

test("service-info shows loading while public instance is pending", async ({ page }) => {
  test.setTimeout(60000);
  let release: (() => void) | undefined;
  const hold = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/v1/instance", async (route) => {
    await hold;
    await route.continue();
  });
  const navigation = page.goto("/service-info");
  await expect(page.getByRole("status")).toContainText("불러오는 중", { timeout: 10000 });
  const required1 = release;
  if (required1 === undefined) {
    throw new Error("Missing fixture value: release");
  }
  required1();
  await navigation;
  await expect(page.getByRole("heading", { name: "서비스 정보" })).toBeVisible();
});

test("service-info surfaces load failure for public instance errors", async ({ page }) => {
  test.setTimeout(60000);
  await page.route("**/api/v1/instance", async (route) => {
    await route.fulfill({
      status: 500,
      contentType: "application/problem+json",
      body: JSON.stringify({
        type: "about:blank",
        title: "Internal Server Error",
        status: 500,
        code: "internal",
      }),
    });
  });
  await page.goto("/service-info");
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.getByRole("button", { name: "다시 시도" })).toBeVisible();
});

test("service-info recovers after manual retry when instance fetch initially fails", async ({
  page,
}) => {
  test.setTimeout(60000);
  let failRequests = true;
  await page.route("**/api/v1/instance", async (route) => {
    if (failRequests) {
      await route.fulfill({
        status: 500,
        contentType: "application/problem+json",
        body: JSON.stringify({
          type: "about:blank",
          title: "Internal Server Error",
          status: 500,
          code: "internal",
        }),
      });
      return;
    }
    await route.continue();
  });
  await page.goto("/service-info");
  await expect(page.getByRole("alert")).toBeVisible();
  failRequests = false;
  await page.getByRole("button", { name: "다시 시도" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "서비스 정보" })).toBeVisible();
  await expect(page.getByRole("button", { name: "다시 시도" })).toHaveCount(0);
});
