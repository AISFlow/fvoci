import fs from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";

const repoRoot = path.resolve(import.meta.dirname, "../../..");
const distLicense = path.join(repoRoot, "apps/web/dist/open-source-licenses.txt");
const distAssets = path.join(repoRoot, "apps/web/dist/assets");

/**
 * Icon sets whose data the built JavaScript inlines (Nuxt UI's bundled icons:
 * `{"prefix":"lucide","icons":{...}}`). They come through a virtual module,
 * so this checks the output rather than the module graph.
 */
function bundledIconSets(): string[] {
  const prefixes = new Set<string>();
  for (const file of fs.readdirSync(distAssets).filter((name) => name.endsWith(".js"))) {
    const code = fs.readFileSync(path.join(distAssets, file), "utf8");
    for (const match of code.matchAll(/"prefix":"([a-z0-9]+(?:-[a-z0-9]+)*)","icons":\{/g)) {
      const prefix = match[1];
      if (prefix === undefined) {
        throw new Error("Bundled icon set must have a prefix");
      }
      prefixes.add(prefix);
    }
  }
  return [...prefixes].sort();
}

/** Build regression guardrails only — not product license policy. */
const FORBIDDEN_NOTICE_PACKAGE_HEADINGS = ["@m2d/", "@playwright/", "vite - "] as const;

async function runSetup(page: Page): Promise<void> {
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15000 });
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("관리자");
  await page.getByLabel("이메일").fill("Admin@Example.COM");
  await page.getByLabel("비밀번호").fill("supersecret1");
  await page.getByLabel("워크스페이스 이름").fill("Acme");
  await page.getByLabel("주소(영문)").fill("acme");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
}

test("open-source-licenses.txt is discoverable, served verbatim, and linked from login footer", async ({
  page,
  browser,
  request,
}) => {
  test.setTimeout(120000);
  expect(fs.existsSync(distLicense), "production build must emit open-source-licenses.txt").toBe(
    true,
  );
  const distBytes = fs.readFileSync(distLicense);
  const distText = distBytes.toString("utf8");
  // Source adaptations are outside Vite's dependency graph; their complete
  // provenance and license text must survive the production build too.
  const sourceNotice = fs.readFileSync(path.join(repoRoot, "apps/web/NOTICE.md"), "utf8").trim();
  expect(distText).toContain(`## FVOCI source: apps/web/NOTICE.md\n\n${sourceNotice}\n`);
  expect(distText).toContain("Copyright (c) 2025 Nuxt UI Templates");
  for (const forbidden of FORBIDDEN_NOTICE_PACKAGE_HEADINGS) {
    expect(distText).not.toMatch(new RegExp(forbidden.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  expect(distText).toMatch(/qrcode-generator - 1\.4\.4/);
  expect(distText).toMatch(/Permission is hereby granted/);
  expect(distText).toMatch(/packages\/editor\/src\/fonts\/NotoSansKR-OFL\.txt/);
  // Every icon set in the bundle is in the notice (the Vue app bundles Lucide).
  const iconSets = bundledIconSets();
  expect(iconSets).toContain("lucide");
  for (const prefix of iconSets) {
    expect(distText, `icon set ${prefix}`).toContain(`## @iconify-json/${prefix} - `);
  }
  expect(distText).toContain("Lucide Icons and Contributors");

  await runSetup(page);

  const indexHtml = await (await request.get("/")).text();
  expect(indexHtml).toContain('rel="license"');
  expect(indexHtml).toContain('href="/open-source-licenses.txt"');

  const served = await request.get("/open-source-licenses.txt");
  expect(served.status()).toBe(200);
  expect(served.headers()["content-type"] ?? "").toMatch(/text\/plain/);
  const servedBody = await served.body();
  expect(Buffer.compare(servedBody, distBytes)).toBe(0);

  const context = await browser.newContext();
  const loginPage = await context.newPage();
  try {
    await loginPage.goto("/login");
    const footer = loginPage.locator(".auth-shell__footer");
    const noticeLink = footer.getByRole("link", { name: "오픈소스 고지" });
    await expect(noticeLink).toHaveAttribute("href", "/open-source-licenses.txt");

    const navigation = loginPage.waitForURL(/\/open-source-licenses\.txt$/);
    await noticeLink.click();
    await navigation;
    await expect(loginPage.locator("body")).toContainText("Kazuhiko Arase");
    await expect(loginPage.locator("body")).toContainText("SIL OPEN FONT LICENSE");
  } finally {
    await context.close();
  }
});
