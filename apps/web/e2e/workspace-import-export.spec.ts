import path from "node:path";
import { crc32 } from "node:zlib";
import { expect, type Page, test } from "@playwright/test";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const importZip = path.resolve(import.meta.dirname, "fixtures/markdown-import.zip");

async function workspaceId(page: Page, slug: string): Promise<string> {
  const res = await page.request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const body = (await res.json()) as { items: { id: string; slug: string }[] };
  const workspace = body.items.find((item) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace?.id ?? "";
}

/** Stored (uncompressed) zip built in the test, so no binary fixture is committed. */
function storedZip(files: Record<string, string | Buffer>): Buffer {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const [name, content] of Object.entries(files)) {
    const data = typeof content === "string" ? Buffer.from(content, "utf8") : content;
    const nameBytes = Buffer.from(name, "utf8");
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBytes, data);
    centrals.push(central, nameBytes);
    offset += 30 + nameBytes.length + data.length;
  }
  const centralSize = centrals.reduce((sum, part) => sum + part.length, 0);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(Object.keys(files).length, 8);
  end.writeUInt16LE(Object.keys(files).length, 10);
  end.writeUInt32LE(centralSize, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, ...centrals, end]);
}

function docx(title: string, body: string): Buffer {
  const ns = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"';
  return storedZip({
    "[Content_Types].xml":
      '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>',
    "word/document.xml": `<?xml version="1.0"?><w:document ${ns}><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>${title}</w:t></w:r></w:p><w:p><w:r><w:t>${body}</w:t></w:r></w:p></w:body></w:document>`,
  });
}

test("owner imports markdown zip and exports document markdown", async ({ page }) => {
  test.setTimeout(120_000);
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

  await page.goto("/w/acme/settings");
  await expect(page.getByLabel("가져올 형식")).toBeVisible();
  await page.locator('input[type="file"]').setInputFiles(importZip);
  await expect(page.getByText("가져오기를 시작했습니다")).toBeVisible({ timeout: 30_000 });

  // Async office-file import: the durable runner converts it; the page polls to completion.
  await page.getByLabel("가져올 형식").selectOption("office-file");
  await page.locator('input[type="file"]').setInputFiles({
    name: "비동기-메모.md",
    mimeType: "text/markdown",
    buffer: Buffer.from("# 비동기 메모\n\n러너가 가져온 본문", "utf8"),
  });
  await expect(page.getByRole("button", { name: "가져오는 중입니다" })).toBeHidden({ timeout: 60_000 });
  await expect(page.getByText("가져오기를 시작했습니다")).toBeVisible();

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: "비동기-메모" })).toBeVisible({ timeout: 15_000 });
  await page.getByRole("link", { name: "e2e-note" }).click();
  await expect(page.getByLabel("문서 제목")).toHaveValue("e2e-note");

  const downloadPromise = page.waitForEvent("download");
  await page.getByRole("button", { name: "Markdown" }).click();
  const download = await downloadPromise;
  const suggested = download.suggestedFilename();
  expect(suggested.endsWith(".md")).toBe(true);
  const text = await download.createReadStream().then(async (stream) => {
    const chunks: Buffer[] = [];
    for await (const chunk of stream) {
      chunks.push(Buffer.from(chunk));
    }
    return Buffer.concat(chunks).toString("utf8");
  });
  expect(text).toContain("E2E note");

  // Office formats convert in the isolated office child: a DOCX becomes a
  // wiki page with its heading and body.
  await page.goto("/w/acme/settings");
  await page.getByLabel("가져올 형식").selectOption("office-file");
  await page.locator('input[type="file"]').setInputFiles({
    name: "워드-회의록.docx",
    mimeType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    buffer: docx("워드 제목", "워드에서 가져온 본문"),
  });
  await expect(page.getByRole("button", { name: "가져오는 중입니다" })).toBeHidden({ timeout: 60_000 });
  await expect(page.getByText("가져오기를 시작했습니다")).toBeVisible();
  await page.goto("/w/acme/wiki");
  await page.getByRole("link", { name: "워드-회의록" }).click();
  await expect(page.getByText("워드에서 가져온 본문")).toBeVisible({ timeout: 15_000 });

  // Notion: a page and a CSV database into the chosen project (the asset
  // becomes an attachment; the web has no document attachment list, so the
  // Rust import suites assert it).
  const id = await workspaceId(page, "acme");
  const createProject = await page.request.post(`/api/v1/workspaces/${id}/projects`, {
    data: { key: "NOT", name: "노션 이관", visibility: "workspace" },
  });
  expect(createProject.status(), await createProject.text()).toBe(201);
  const project = (await createProject.json()) as { id: string };
  await page.goto("/w/acme/settings");
  await page.getByLabel("가져올 형식").selectOption("notion-zip");
  await page.getByLabel("대상 프로젝트").selectOption({ label: "노션 이관" });
  await page.locator('input[type="file"]').setInputFiles({
    name: "notion-export.zip",
    mimeType: "application/zip",
    buffer: storedZip({
      "Export/로드맵 0123456789abcdef.md": "# 로드맵\n\n노션 본문",
      "Export/로드맵 0123456789abcdef/메모.txt": "첨부 메모",
      "Export/로드맵 0123456789abcdef/할 일 89abcdef01.csv": "이름,상태\n노션에서 온 태스크,완료\n",
    }),
  });
  await expect(page.getByRole("button", { name: "가져오는 중입니다" })).toBeHidden({ timeout: 60_000 });
  await expect(page.getByText("가져오기를 시작했습니다")).toBeVisible();
  const tasksRes = await page.request.get(`/api/v1/workspaces/${id}/projects/${project.id}/tasks`);
  expect(tasksRes.ok(), await tasksRes.text()).toBe(true);
  const tasks = (await tasksRes.json()) as { items: { title: string }[] };
  expect(tasks.items.map((task) => task.title)).toEqual(["노션에서 온 태스크"]);
  await page.goto("/w/acme/wiki");
  await page.getByRole("link", { name: "로드맵" }).click();
  await expect(page.getByText("노션 본문")).toBeVisible({ timeout: 15_000 });
});
