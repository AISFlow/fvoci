// Wiki document chrome already live on the Vue page (#262): star, share,
// comments, tags, markdown export, revisions. The collab body suite stays in
// workspace-wiki-vue-flow.spec.ts. This group hits the production dist and
// the real Rust API/DB/collab the way a user would, and does not require the
// public /s/:token page to be Vue (that route is still a React boundary).
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { login, watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial", timeout: 90_000 });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "vchrome",
  workspaceName: "Vue Wiki Chrome E2E",
};

async function ensureSetup(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if ((await page.getByRole("button", { name: "시작하기" }).count()) > 0) {
    await page.getByLabel("성").fill(admin.familyName);
    await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
    await page.getByLabel("이메일").fill(admin.email);
    await page.getByLabel("비밀번호").fill(admin.password);
    await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
    await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
    return;
  }
  if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, admin.email, admin.password);
  }
}

async function workspaceId(request: APIRequestContext): Promise<string> {
  const res = await request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const workspace = (await res.json()).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  expect(workspace).toBeTruthy();
  return workspace.id;
}

type WikiDoc = { id: string; number: number; path: string };

async function createDoc(
  request: APIRequestContext,
  wsId: string,
  title: string,
): Promise<WikiDoc> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title },
  });
  expect(res.status(), await res.text()).toBe(201);
  const doc = (await res.json()) as { id: string; number: number };
  return { ...doc, path: `/w/${admin.workspaceSlug}/WIKI-${doc.number}` };
}

async function openDoc(page: Page, doc: WikiDoc): Promise<void> {
  const navigation = await page.goto(doc.path);
  expect(navigation?.status()).toBe(200);
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await expect(page.getByTestId(`document-WIKI-${doc.number}`)).toBeVisible();
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
}

test.beforeEach(async ({ page }) => {
  await ensureSetup(page);
});

test("star toggle persists after reload", async ({ page }) => {
  const csp = watchCspViolations(page);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "별 문서");
  await openDoc(page, doc);

  await page.getByRole("button", { name: "즐겨찾기 추가" }).click();
  await expect(page.getByRole("button", { name: "즐겨찾기 해제" })).toBeVisible();
  const starred = await page.request.get(`/api/v1/workspaces/${wsId}/stars`);
  expect(starred.ok()).toBe(true);
  expect(
    ((await starred.json()) as { items: { targetId: string }[] }).items.map(
      (item) => item.targetId,
    ),
  ).toEqual([doc.id]);

  await page.reload();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await expect(page.getByRole("button", { name: "즐겨찾기 해제" })).toBeVisible();
  const afterReload = await page.request.get(`/api/v1/workspaces/${wsId}/stars`);
  expect(
    ((await afterReload.json()) as { items: { targetId: string }[] }).items.map(
      (item) => item.targetId,
    ),
  ).toEqual([doc.id]);
  expect(csp).toEqual([]);
});

test("share dialog creates, copies and revokes a link", async ({ page }) => {
  const csp = watchCspViolations(page);
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "공유 문서");
  await openDoc(page, doc);

  const policy = await page.request.patch("/api/v1/admin/instance-settings", {
    data: { share: { enabled: true, defaultExpiresDays: 14, maxExpiresDays: 30 } },
  });
  expect(policy.status()).toBe(200);

  await page.getByRole("button", { name: "공유 링크" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("공유 링크가 없습니다")).toBeVisible();
  const expires = dialog.getByLabel("만료 기간");
  await expect(expires).toBeEnabled();
  await expect(expires).toHaveValue("14");
  await expires.selectOption("30");
  await dialog.getByRole("button", { name: "공유 링크", exact: true }).click();
  const urlBox = dialog.getByRole("textbox", { name: "공유 링크" });
  await expect(urlBox).toHaveValue(/\/s\/[A-Za-z0-9_-]+$/);
  const shareUrl = await urlBox.inputValue();
  await dialog.getByRole("button", { name: "복사" }).click();
  await expect(dialog.getByRole("button", { name: "복사됨" })).toBeVisible();

  // Public /s/:token may still be the React page; only the token itself is required here.
  const sharePath = new URL(shareUrl).pathname;
  const shell = await page.request.get(sharePath);
  expect(shell.status()).toBe(200);

  page.once("dialog", (confirm) => {
    expect(confirm.message()).toContain("공유 링크를 해제할까요?");
    void confirm.accept();
  });
  await dialog.getByRole("button", { name: "해제" }).click();
  await expect(dialog.getByText("공유 링크가 없습니다")).toBeVisible();
  expect(csp).toEqual([]);
});

test("comments: add a comment and resolve it", async ({ page }) => {
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "댓글 문서");
  await openDoc(page, doc);

  const panel = page.getByTestId("document-comments");
  await expect(panel.getByRole("heading", { name: "댓글" })).toBeVisible();
  const compose = panel.locator("[data-comment-compose] textarea");
  await compose.fill("크롬 댓글입니다");
  await panel.getByRole("button", { name: "등록" }).click();
  await expect(panel.getByText("크롬 댓글입니다")).toBeVisible();
  await panel.getByRole("button", { name: "해결" }).click();
  await expect(panel.getByRole("button", { name: "다시 열기" })).toBeVisible();
});

test("tags bar: add and remove a tag", async ({ page }) => {
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "태그 문서");
  await openDoc(page, doc);

  const tagsBar = page.getByTestId("document-tags-bar");
  await tagsBar.getByRole("button", { name: "+ 태그" }).click();
  await tagsBar.getByRole("textbox", { name: "문서 태그" }).fill("크롬태그");
  await tagsBar.getByRole("button", { name: "「크롬태그」 만들기" }).click();
  await expect(tagsBar.getByRole("button", { name: "태그 제거: 크롬태그" })).toBeVisible();
  await expect
    .poll(async () => {
      const res = await page.request.get(`/api/v1/workspaces/${wsId}/documents/${doc.id}/tags`);
      return ((await res.json()) as { items: { name: string }[] }).items.map((tag) => tag.name);
    })
    .toEqual(["크롬태그"]);

  await tagsBar.getByRole("button", { name: "태그 제거: 크롬태그" }).click();
  await expect(tagsBar.getByRole("button", { name: "태그 제거: 크롬태그" })).toHaveCount(0);
  await expect
    .poll(async () => {
      const res = await page.request.get(`/api/v1/workspaces/${wsId}/documents/${doc.id}/tags`);
      return ((await res.json()) as { items: { name: string }[] }).items;
    })
    .toEqual([]);
});

test("export menu downloads markdown", async ({ page }) => {
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "보내기 문서");
  await openDoc(page, doc);
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await expect(editor).toBeVisible();
  await editor.click();
  await page.keyboard.type("내보내는 본문");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });

  const downloadPromise = page.waitForEvent("download");
  await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
  await page.getByRole("button", { name: "Markdown" }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("보내기 문서.md");
  const text = await download.createReadStream().then(async (stream) => {
    const chunks: Buffer[] = [];
    for await (const chunk of stream) chunks.push(Buffer.from(chunk));
    return Buffer.concat(chunks).toString("utf8");
  });
  expect(text).toContain("내보내는 본문");
});

test("revisions: save a revision and restore it", async ({ page }) => {
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "리비전 문서");
  const bodyUrl = `/api/v1/workspaces/${wsId}/documents/${doc.id}/body`;
  await openDoc(page, doc);
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await expect(editor).toBeVisible();
  await editor.click();
  await page.keyboard.type("첫 번째 버전");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });

  const created = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().endsWith(`/documents/${doc.id}/revisions`),
  );
  await page.getByTestId("revision-history").click();
  const before = await page.getByTestId("revision-item").count();
  await page.getByTestId("revision-save").click();
  expect((await created).status()).toBe(201);
  await expect(page.getByTestId("revision-item")).toHaveCount(before + 1);

  await page.getByTestId("revision-history").click();
  await expect(page.getByTestId("revision-save")).toHaveCount(0);
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.type("두 번째 버전");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect
    .poll(async () => JSON.stringify((await (await page.request.get(bodyUrl)).json()).contentJson))
    .toContain("두 번째 버전");

  await page.getByTestId("revision-history").click();
  await page.getByTestId("revision-restore").first().click();
  await page.getByTestId("revision-restore-confirm").click();
  await expect(editor).toContainText("첫 번째 버전", { timeout: 15_000 });
  await expect
    .poll(async () => JSON.stringify((await (await page.request.get(bodyUrl)).json()).contentJson))
    .toContain("첫 번째 버전");
});

test("long Korean wiki title wraps, metadata leaves body visible and Enter keeps single-line title persistence", async ({
  page,
}) => {
  const wsId = await workspaceId(page.request);
  const title =
    "한국어 협업 문서 제목이 길어질 때 탐색과 편집 작업을 안정적으로 유지하는 주간 업무 기록 및 검토 결과";
  const doc = await createDoc(page.request, wsId, title);
  const bodyUrl = `/api/v1/workspaces/${wsId}/documents/${doc.id}/body`;
  expect(
    (
      await page.request.put(bodyUrl, {
        data: {
          contentMd:
            "첫 번째 업무 본문입니다.\n\n두 번째 업무 본문입니다.\n\n세 번째 업무 본문입니다.",
        },
      })
    ).ok(),
  ).toBe(true);
  await page.setViewportSize({ width: 390, height: 844 });
  await openDoc(page, doc);
  const field = page.getByLabel("문서 제목");
  await expect(field).toHaveValue(title);
  await expect
    .poll(async () => field.evaluate((e) => e.scrollHeight <= e.clientHeight + 1))
    .toBe(true);
  expect(await field.evaluate((e) => e.scrollWidth)).toBeLessThanOrEqual(
    await field.evaluate((e) => e.clientWidth),
  );
  expect(
    await page
      .locator(".ProseMirror > p")
      .nth(2)
      .evaluate((e) => e.getBoundingClientRect().bottom),
  ).toBeLessThan(844);
  await page.setViewportSize({ width: 1280, height: 720 });
  await expect
    .poll(async () => field.evaluate((e) => e.scrollHeight <= e.clientHeight + 1))
    .toBe(true);
  expect(
    await page
      .locator(".ProseMirror > p")
      .nth(2)
      .evaluate((e) => e.getBoundingClientRect().bottom),
  ).toBeLessThan(720);
  const options = page.getByRole("button", { name: "문서 옵션", exact: true });
  await expect(options).toHaveAttribute("aria-expanded", "false");
  await options.focus();
  await options.press("Enter");
  await expect(options).toHaveAttribute("aria-expanded", "true");
  const icon = page.getByLabel("아이콘");
  await icon.fill("📚");
  await icon.press("Tab");
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/v1/workspaces/${wsId}/documents/${doc.id}`)).json())
          .icon,
    )
    .toBe("📚");
  await page.getByLabel("문서 상태").selectOption("published");
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/v1/workspaces/${wsId}/documents/${doc.id}`)).json())
          .status,
    )
    .toBe("published");
  await expect(icon).toBeEnabled();
  await icon.focus();
  await icon.press("Escape");
  await expect(options).toBeFocused();
  await expect(options).toHaveAttribute("aria-expanded", "false");
  await expect(icon).toBeHidden();
  await field.fill("한국어 제목\n붙여넣기");
  await expect(field).toHaveValue("한국어 제목붙여넣기");
  await field.press("Enter");
  await expect
    .poll(
      async () =>
        (await (await page.request.get(`/api/v1/workspaces/${wsId}/documents/${doc.id}`)).json())
          .title,
    )
    .toBe("한국어 제목붙여넣기");
  await page.reload();
  await expect(field).toHaveValue("한국어 제목붙여넣기");
});
