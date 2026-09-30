import { expect, test, type Page } from "@playwright/test";
import { readJson, flowSchemas, watchCspViolations } from "./helpers";
import { expectVueViewer } from "./viewer-app";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

// 1x1 opaque PNG.
const PNG_BYTES = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

async function uploadDocumentAttachment(
  page: Page,
  wsId: string,
  documentId: string,
  name: string,
  bytes: Buffer,
): Promise<string> {
  const uploadRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/documents/${documentId}/uploads`,
    { data: { name, sizeBytes: bytes.length } },
  );
  expect(uploadRes.ok(), await uploadRes.text()).toBeTruthy();
  const upload = (await readJson(uploadRes, flowSchemas.upload)) as {
    attachmentId: string;
    partSizeBytes: number;
    parts: Array<{
      partNumber: number;
      url: string;
    }>;
  };
  const parts: {
    partNumber: number;
    etag: string;
  }[] = [];
  for (const part of upload.parts) {
    const put = await page.request.put(part.url, {
      headers: { "content-type": "application/octet-stream" },
      data: bytes.subarray(
        (part.partNumber - 1) * upload.partSizeBytes,
        part.partNumber * upload.partSizeBytes,
      ),
    });
    expect(put.ok(), await put.text()).toBeTruthy();
    const etag = put.headers()["etag"];
    expect(etag).toBeTruthy();
    const required1 = etag;
    if (required1 === undefined) {
      throw new Error("Missing fixture value: etag");
    }
    parts.push({ partNumber: part.partNumber, etag: required1 });
  }
  const completeRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/attachments/${upload.attachmentId}/complete`,
    { data: { parts } },
  );
  expect(completeRes.ok(), await completeRes.text()).toBeTruthy();
  return upload.attachmentId;
}

test("anonymous share attachment view: text, image and download inside the share; denied outside and after revoke", async ({
  page,
  browser,
}) => {
  test.setTimeout(90000);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15000 });
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
  const workspaces = (await readJson(workspacesRes, flowSchemas.workspaces)) as {
    items: {
      id: string;
      slug: string;
    }[];
  };
  const required2 = workspaces.items.find((item) => item.slug === owner.workspaceSlug);
  if (required2 === undefined) {
    throw new Error(
      "Missing fixture value: workspaces.items.find((item) => item.slug === owner.workspaceSlug)",
    );
  }
  const wsId = required2.id;
  const createDoc = async (title: string, parentId: string | null) => {
    const res = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
      data: { parentId, title },
    });
    expect(res.status()).toBe(201);
    return (
      (await readJson(res, flowSchemas.document)) as {
        id: string;
      }
    ).id;
  };
  const rootId = await createDoc("공유 첨부 루트", null);
  const childId = await createDoc("공유 첨부 하위", rootId);
  const outsideId = await createDoc("공유 밖 문서", null);

  const textBody = "공유 첨부 본문 한글 ✅\n둘째 줄\n";
  const textBytes = Buffer.from(textBody, "utf8");
  const binBytes = Buffer.from([0, 1, 2, 3, 250, 251, 252, 253]);
  // The text file sits in the shared subtree, not on the share root itself.
  const textId = await uploadDocumentAttachment(page, wsId, childId, "메모.txt", textBytes);
  const imageId = await uploadDocumentAttachment(page, wsId, rootId, "pixel.png", PNG_BYTES);
  const binId = await uploadDocumentAttachment(page, wsId, rootId, "blob.bin", binBytes);
  const outsideTextId = await uploadDocumentAttachment(
    page,
    wsId,
    outsideId,
    "밖.txt",
    Buffer.from("공유 밖 비밀", "utf8"),
  );

  const shareRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/documents/${rootId}/share-links`,
    { data: { expiresInDays: 7 } },
  );
  expect(shareRes.status(), await shareRes.text()).toBe(201);
  const share = (await readJson(shareRes, flowSchemas.share)) as {
    id: string;
    url: string;
  };
  const sharePath = new URL(share.url).pathname;
  const required3 = sharePath.split("/")[2];
  if (required3 === undefined) {
    throw new Error('Missing fixture value: sharePath.split("/")[2]');
  }
  const token = required3;
  expect(token.length).toBeGreaterThan(10);
  const viewPath = (attachmentId: string) => `${sharePath}/attachments/${attachmentId}/view`;

  // The deep link is served by the SPA shell with the global no-referrer policy.
  const shell = await page.request.get(viewPath(textId));
  expect(shell.status()).toBe(200);
  expect(shell.headers()["referrer-policy"]).toBe("no-referrer");

  // Anonymous reader in a fresh context without cookies.
  const anon = await browser.newContext({ acceptDownloads: true });
  expect(await anon.cookies()).toEqual([]);
  const reader = await anon.newPage();
  const readerCspViolations = watchCspViolations(reader);
  const apiPaths: string[] = [];
  reader.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.startsWith("/api/")) {
      apiPaths.push(path);
    }
  });
  const consoleLines: string[] = [];
  reader.on("console", (message) => consoleLines.push(message.text()));

  // Text: bytes rendered, search chunk highlighted, download keeps the original.
  await reader.goto(`${viewPath(textId)}?chunk=0`);
  const viewer = reader.locator("[data-attachment-viewer]");
  await expect(viewer.locator(".attachment-viewer__name")).toHaveText("메모.txt");
  await expect(viewer.locator("pre.attachment-viewer__text")).toContainText(
    "공유 첨부 본문 한글 ✅",
  );
  await expect(viewer.locator("pre mark")).toContainText("공유 첨부 본문");
  await expectVueViewer(reader);
  await reader.reload();
  await expect(viewer.locator("pre mark")).toContainText("공유 첨부 본문");
  await expectVueViewer(reader);
  await expect(reader.getByRole("link", { name: "편집" })).toHaveCount(0);
  await expect(reader.getByRole("button", { name: /편집/ })).toHaveCount(0);
  const downloadLink = viewer.getByRole("link", { name: "다운로드" });
  await expect(downloadLink).toHaveAttribute(
    "href",
    `/api/v1/share/${token}/attachments/${textId}/download`,
  );
  const downloadEvent = reader.waitForEvent("download");
  await downloadLink.click();
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toBe("메모.txt");
  const downloaded = await download.createReadStream().then(async (stream) => {
    const chunks: Buffer[] = [];
    for await (const chunk of stream) chunks.push(chunk as Buffer);
    return Buffer.concat(chunks);
  });
  expect(downloaded.equals(textBytes)).toBe(true);

  // Image: rendered from the share download URL, never a session URL.
  await reader.goto(viewPath(imageId));
  const image = reader.locator("img.attachment-viewer__image");
  await expect(image).toHaveAttribute(
    "src",
    `/api/v1/share/${token}/attachments/${imageId}/download`,
  );
  await expect.poll(() => image.evaluate((el) => (el as HTMLImageElement).naturalWidth)).toBe(1);

  // Other kinds keep the download choice.
  await reader.goto(viewPath(binId));
  await expect(reader.getByRole("alert")).toHaveText(
    "이 파일을 뷰어로 열 수 없습니다. 원본을 다운로드하세요.",
  );
  await expect(
    reader.locator("[data-attachment-viewer]").getByRole("link", { name: "다운로드" }).first(),
  ).toHaveAttribute("href", `/api/v1/share/${token}/attachments/${binId}/download`);

  // Outside the shared subtree and unknown ids: the same privacy-preserving denial.
  const notFound = "접근 권한이 없거나 존재하지 않는 항목입니다.";
  await reader.goto(viewPath(outsideTextId));
  await expect(reader.getByRole("alert")).toHaveText(notFound);
  await expect(reader.getByText("공유 밖 비밀")).toHaveCount(0);
  await expect(reader.getByRole("button", { name: "다시 시도" })).toHaveCount(0);
  await reader.goto(viewPath("0190c3b8-0000-7000-8000-000000000000"));
  await expect(reader.getByRole("alert")).toHaveText(notFound);

  // Revoked: the same deep link is denied and the bytes are gone.
  const revoke = await page.request.delete(`/api/v1/workspaces/${wsId}/share-links/${share.id}`);
  expect(revoke.ok(), await revoke.text()).toBeTruthy();
  await reader.goto(viewPath(textId));
  await expect(reader.getByRole("alert")).toHaveText(notFound);
  await expect(reader.locator("pre.attachment-viewer__text")).toHaveCount(0);
  const revokedBytes = await reader.request.get(
    `/api/v1/share/${token}/attachments/${textId}/download`,
  );
  expect(revokedBytes.status()).toBe(404);

  // Only the share API was called; no session attachment route, no token in the console.
  expect(apiPaths.length).toBeGreaterThan(0);
  for (const path of apiPaths) {
    expect(path.startsWith("/api/v1/share/")).toBe(true);
    expect(path).not.toContain("/workspaces/");
  }
  for (const line of consoleLines) {
    expect(line).not.toContain(token);
  }
  expect(readerCspViolations).toEqual([]);
  await anon.close();
});
