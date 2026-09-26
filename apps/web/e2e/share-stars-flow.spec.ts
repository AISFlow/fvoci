import { expect, test } from "@playwright/test";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

test("owner stars and shares a wiki document; the public link needs no session and dies on revoke", async ({
  page,
  browser,
}) => {
  test.setTimeout(90_000);

  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await page.getByLabel("성").fill(owner.familyName);
  await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
  await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspaces = (await workspacesRes.json()) as { items: { id: string; slug: string }[] };
  const wsId = workspaces.items.find((item) => item.slug === owner.workspaceSlug)!.id;

  const rootTitle = "공유 루트 문서";
  const childTitle = "공유 하위 문서";
  const bodyText = "공개로 읽는 본문";
  const rootRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: rootTitle },
  });
  expect(rootRes.status()).toBe(201);
  const root = (await rootRes.json()) as { id: string; number: number };
  const childRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: root.id, title: childTitle },
  });
  expect(childRes.status()).toBe(201);

  // Body through the real collab editor, then a durable persist.
  await page.goto(`/w/acme/WIKI-${root.number}`);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await expect(editor).toBeVisible();
  await editor.click();
  await page.keyboard.type(bodyText);
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });

  // Star toggle, then the starred and recent lists on the workspace home.
  await page.getByRole("button", { name: "즐겨찾기 추가" }).click();
  await expect(page.getByRole("button", { name: "즐겨찾기 해제" })).toBeVisible();
  const starsRes = await page.request.get(`/api/v1/workspaces/${wsId}/stars`);
  expect(((await starsRes.json()) as { items: { targetId: string }[] }).items.map((s) => s.targetId)).toEqual([
    root.id,
  ]);

  await page.getByRole("link", { name: "홈", exact: true }).click();
  await expect(page).toHaveURL(/\/w\/acme$/);
  const starred = page.getByRole("region", { name: "즐겨찾기" });
  await expect(starred.getByRole("link", { name: new RegExp(rootTitle) })).toBeVisible();
  await expect(starred.getByRole("link", { name: new RegExp(childTitle) })).toHaveCount(0);
  const recent = page.getByRole("region", { name: "최근" });
  await expect(recent.getByRole("link", { name: new RegExp(rootTitle) })).toBeVisible();
  await expect(recent.getByRole("link", { name: new RegExp(childTitle) })).toBeVisible();

  // The instance share policy drives the dialog's expiry choices.
  const policy = await page.request.patch("/api/v1/admin/instance-settings", {
    data: { share: { enabled: true, defaultExpiresDays: 14, maxExpiresDays: 30 } },
  });
  expect(policy.status()).toBe(200);

  // Share dialog: create a 30-day link and read the one-time URL.
  await starred.getByRole("link", { name: new RegExp(rootTitle) }).click();
  await expect(page).toHaveURL(new RegExp(`/w/acme/WIKI-${root.number}$`));
  await page.getByRole("button", { name: "공유 링크" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("공유 링크가 없습니다")).toBeVisible();
  const expires = dialog.getByLabel("만료 기간");
  await expect(expires).toHaveValue("14");
  expect(await expires.locator("option").evaluateAll((els) => els.map((el) => (el as HTMLOptionElement).value))).toEqual([
    "7",
    "14",
    "30",
  ]);
  await expires.selectOption("30");
  await dialog.getByRole("button", { name: "공유 링크", exact: true }).click();
  const urlBox = dialog.getByRole("textbox", { name: "공유 링크" });
  await expect(urlBox).toHaveValue(/\/s\/[A-Za-z0-9_-]+$/);
  const shareUrl = await urlBox.inputValue();
  await expect(dialog.getByRole("button", { name: "해제" })).toHaveCount(1);
  const sharePath = new URL(shareUrl).pathname;

  // The /s/:token shell carries the share's title and excerpt for link unfurlers.
  const shell = await page.request.get(sharePath);
  expect(shell.status()).toBe(200);
  expect(shell.headers()["x-robots-tag"]).toBe("noindex");
  expect(shell.headers()["cache-control"]).toBe("private, no-store");
  const shellHtml = await shell.text();
  expect(shellHtml).toContain(`<title>${rootTitle}</title>`);
  expect(shellHtml).toContain(`<meta property="og:title" content="${rootTitle}"/>`);
  expect(shellHtml).toMatch(new RegExp(`<meta property="og:description" content="[^"]*${bodyText}`));

  // Anonymous reader in a fresh context without cookies.
  const anon = await browser.newContext();
  expect(await anon.cookies()).toEqual([]);
  const reader = await anon.newPage();
  const apiPaths: string[] = [];
  reader.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.startsWith("/api/")) apiPaths.push(path);
  });
  const publicUrl = new URL(sharePath, page.url()).toString();
  await reader.goto(publicUrl);
  await expect(reader).toHaveURL(publicUrl);
  await expect(reader.getByRole("heading", { level: 1, name: rootTitle })).toBeVisible();
  await expect(reader.getByTestId("share-body")).toContainText(bodyText);
  await expect(reader.getByText("읽기 전용")).toBeVisible();
  const tree = reader.getByRole("navigation", { name: "문서" });
  await expect(tree.getByRole("button", { name: rootTitle })).toBeVisible();
  await tree.getByRole("button", { name: childTitle }).click();
  await expect(reader.getByRole("heading", { level: 1, name: childTitle })).toBeVisible();
  await expect(reader.getByTestId("share-body")).not.toContainText(bodyText);
  await tree.getByRole("button", { name: rootTitle }).click();
  await expect(reader.getByTestId("share-body")).toContainText(bodyText);
  // The anonymous reader downloads the shown document as PDF.
  const pdfDownload = reader.waitForEvent("download");
  await reader.getByRole("button", { name: "PDF" }).click();
  const pdf = await pdfDownload;
  expect(pdf.suggestedFilename()).toBe(`${rootTitle}.pdf`);
  const pdfBytes = await pdf.createReadStream().then(async (stream) => {
    const chunks: Buffer[] = [];
    for await (const chunk of stream) chunks.push(chunk as Buffer);
    return Buffer.concat(chunks);
  });
  expect(pdfBytes.subarray(0, 5).toString("latin1")).toBe("%PDF-");
  expect(apiPaths.length).toBeGreaterThan(0);
  for (const path of apiPaths) {
    expect(path.startsWith("/api/v1/share/")).toBe(true);
  }

  // Revoke from the dialog; the public page turns into the invalid state.
  page.once("dialog", (confirm) => {
    expect(confirm.message()).toContain("공유 링크를 해제할까요?");
    void confirm.accept();
  });
  await dialog.getByRole("button", { name: "해제" }).click();
  await expect(dialog.getByText("공유 링크가 없습니다")).toBeVisible();

  await reader.reload();
  await expect(reader.getByRole("alert")).toHaveText("공유 링크가 만료되었습니다");
  await expect(reader).toHaveURL(publicUrl);
  await anon.close();

  // With sharing disabled by the admin, the dialog says so and cannot create.
  const disabled = await page.request.patch("/api/v1/admin/instance-settings", {
    data: { share: { enabled: false, defaultExpiresDays: 14, maxExpiresDays: 30 } },
  });
  expect(disabled.status()).toBe(200);
  await page.reload();
  await page.getByRole("button", { name: "공유 링크" }).click();
  const off = page.getByRole("dialog");
  await expect(off.getByRole("alert")).toHaveText("관리자가 공개 공유를 비활성화했습니다.");
  await expect(off.getByRole("button", { name: "공유 링크", exact: true })).toBeDisabled();
  await expect(off.getByLabel("만료 기간")).toBeDisabled();
});
