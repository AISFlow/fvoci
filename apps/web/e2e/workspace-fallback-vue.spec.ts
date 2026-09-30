import { expect, test } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });
const owner = { email: "fallback@example.com", password: "fallbackpass123" };
let workspaceId: string;

test("generic workspace refs keep the real setup gate before canonicalization", async ({ page }) => {
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
  workspaceId = (await (await page.request.get("/api/v1/me/workspaces")).json()).items[0].id;
  const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, { data: { key: "OPS", name: "Canonical project", visibility: "workspace" } });
  expect(response.status()).toBe(201);
  const project = await response.json();
  expect((await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`, { data: { title: "Canonical task" } })).status()).toBe(201);
  expect((await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, { data: { title: "Canonical wiki", parentId: null } })).status()).toBe(201);
});

test("encoded, trimmed and NFKC project refs canonicalize once inside Vue with query and fragment", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await page.addInitScript(() => {
    if (window.top === window) sessionStorage.setItem("fallbackBoots", String(Number(sessionStorage.getItem("fallbackBoots") ?? 0) + 1));
  });
  for (const ref of ["%4FPS", "%20ops%20", "%EF%BC%AF%EF%BC%B0%EF%BC%B3"]) {
    const before = await page.evaluate(() => Number(sessionStorage.getItem("fallbackBoots") ?? 0));
    await page.goto(`/w/fallback/${ref}?from=encoded%20ref#overview`);
    await expect(page).toHaveURL(/\/OPS\?from=encoded%20ref#overview$/);
    await expect(page.getByRole("heading", { name: "Canonical project", exact: true })).toBeVisible();
    await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
    expect(await page.evaluate(() => Number(sessionStorage.getItem("fallbackBoots")))).toBe(before + 1);
  }
  for (const [ref, title, target] of [["%4FPS-2", "Canonical task", "OPS-2"], ["%57IKI-1", "Canonical wiki", "WIKI-1"]]) {
    const before = await page.evaluate(() => Number(sessionStorage.getItem("fallbackBoots")));
    await page.goto(`/w/fallback/${ref}?from=item#document-comments`);
    await expect(page).toHaveURL(new RegExp(`/${target}\\?from=item#document-comments$`));
    await expect(page.getByRole("heading", { name: title, exact: true })).toBeVisible();
    expect(await page.evaluate(() => Number(sessionStorage.getItem("fallbackBoots")))).toBe(before + 1);
  }
});

test("invalid authorized refs retain the workspace shell; unknown nested paths go home without a reload loop", async ({ page }) => {
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

test("fallback refs enforce real missing-session and inaccessible-workspace gates", async ({ page, browser }) => {
  await login(page, owner.email, owner.password);
  await page.goto("/w/absent/%20OPS%20?from=denied#overview");
  await expect(page).toHaveURL(/\/\?denied=workspace$/);
  const context = await browser.newContext();
  try {
    const signedOut = await context.newPage();
    await signedOut.goto(`${new URL(page.url()).origin}/w/fallback/%20OPS%20?from=signed-out#overview`);
    await expect(signedOut).toHaveURL(/\/login\?returnTo=/);
    expect(new URL(signedOut.url()).searchParams.get("returnTo")).toBe("/w/fallback/%20OPS%20?from=signed-out#overview");
    await expect(signedOut.getByLabel("이메일")).toBeVisible();
  } finally {
    await context.close();
  }
});
