import { expect, test } from "@playwright/test";
import { createE2eUser, login, logout } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const outsider = {
  email: "search-outsider@example.com",
  password: "outsiderpass1",
  givenName: "외부",
  familyName: "검색",
};

type SearchHit = { type: string; id: string; title: string };

async function searchItems(
  page: { request: { get: (url: string) => Promise<{ ok: () => boolean; json: () => Promise<unknown> }> } },
  url: string,
): Promise<SearchHit[]> {
  const res = await page.request.get(url);
  if (!res.ok()) return [];
  const body = (await res.json()) as { items?: SearchHit[] };
  return body.items ?? [];
}

test("workspace and global search find a document, task, comment, and attachment", async ({
  page,
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

  createE2eUser(outsider.email, outsider.password, outsider.givenName, {
    familyName: outsider.familyName,
  });

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspaces = (await workspacesRes.json()) as { items: { id: string; slug: string }[] };
  const workspace = workspaces.items.find((item) => item.slug === owner.workspaceSlug);
  expect(workspace).toBeTruthy();
  const wsId = workspace!.id;

  const stamp = Date.now();
  const token = `srch${stamp}`;
  const documentTitle = `${token} 문서`;
  const taskTitle = `${token} 태스크`;
  const commentBody = `${token} 댓글본문`;
  const attachmentName = `${token}-note.txt`;

  const docRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: documentTitle },
  });
  expect(docRes.ok(), await docRes.text()).toBeTruthy();
  const createdDoc = (await docRes.json()) as { id: string; displayId: string };
  const documentId = createdDoc.id;

  await page.goto("/w/acme/projects");
  await page.getByRole("button", { name: "새 프로젝트" }).click();
  await page.getByLabel("키").fill("SRC");
  await page.getByLabel("이름", { exact: true }).fill("탐색기");
  await page.getByLabel("공개 범위").selectOption("workspace");
  await page.getByRole("dialog").getByRole("button", { name: "새 프로젝트" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/SRC\/tasks$/);

  const projectsRes = await page.request.get(`/api/v1/workspaces/${wsId}/projects`);
  expect(projectsRes.ok()).toBe(true);
  const projects = (await projectsRes.json()) as { items: Array<{ id: string; key: string }> };
  const projectId = projects.items.find((item) => item.key === "SRC")?.id;
  expect(projectId).toBeTruthy();

  const taskRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`, {
    data: { title: taskTitle },
  });
  expect(taskRes.status()).toBe(201);
  await taskRes.json();

  const commentRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/documents/${documentId}/comments`,
    { data: { body: commentBody } },
  );
  expect(commentRes.status(), await commentRes.text()).toBe(201);

  const bytes = Buffer.from(`${token} attachment-body\n`, "utf8");
  const uploadRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/documents/${documentId}/uploads`,
    { data: { name: attachmentName, sizeBytes: bytes.length } },
  );
  expect(uploadRes.ok(), await uploadRes.text()).toBeTruthy();
  const upload = (await uploadRes.json()) as {
    attachmentId: string;
    partSizeBytes: number;
    parts: Array<{ partNumber: number; url: string }>;
  };
  const parts: { partNumber: number; etag: string }[] = [];
  for (const part of upload.parts) {
    const put = await page.request.put(part.url, {
      headers: { "content-type": "application/octet-stream" },
      data: bytes.subarray(
        (part.partNumber - 1) * upload.partSizeBytes,
        part.partNumber * upload.partSizeBytes,
      ),
    });
    expect(put.ok(), await put.text()).toBeTruthy();
    const etag = put.headers()["etag"] ?? put.headers()["ETag"];
    expect(etag).toBeTruthy();
    parts.push({ partNumber: part.partNumber, etag: etag! });
  }
  const completeRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/attachments/${upload.attachmentId}/complete`,
    { data: { parts } },
  );
  expect(completeRes.ok(), await completeRes.text()).toBeTruthy();

  const waitForHit = async (url: string, type: string, title: string) => {
    await expect
      .poll(
        async () => {
          const items = await searchItems(page, url);
          return items.some((item) => item.type === type && item.title === title);
        },
        { timeout: 30_000 },
      )
      .toBe(true);
  };

  const workspaceQ = `/api/v1/workspaces/${wsId}/search?q=${encodeURIComponent(token)}`;
  await waitForHit(`${workspaceQ}&type=document`, "document", documentTitle);
  await waitForHit(`${workspaceQ}&type=task`, "task", taskTitle);
  await waitForHit(`${workspaceQ}&type=comment`, "comment", documentTitle);
  await waitForHit(`${workspaceQ}&type=attachment`, "attachment", attachmentName);

  const globalQ = `/api/v1/search?q=${encodeURIComponent(token)}`;
  await waitForHit(`${globalQ}&type=document`, "document", documentTitle);
  await waitForHit(`${globalQ}&type=task`, "task", taskTitle);
  await waitForHit(`${globalQ}&type=comment`, "comment", documentTitle);
  await waitForHit(`${globalQ}&type=attachment`, "attachment", attachmentName);

  await page.goto(`/w/acme/search?q=${encodeURIComponent(token)}`);
  const results = page.getByRole("region", { name: "검색" });
  await expect(results.getByText(documentTitle).first()).toBeVisible({ timeout: 10_000 });
  await expect(results.getByText(taskTitle)).toBeVisible();
  await expect(results.getByText(attachmentName)).toBeVisible();

  await logout(page);
  await login(page, outsider.email, outsider.password);
  const outsiderWorkspace = await page.request.get(
    `/api/v1/workspaces/${wsId}/search?q=${encodeURIComponent(token)}`,
  );
  expect(outsiderWorkspace.status()).toBe(404);
  const outsiderGlobal = await searchItems(page, `/api/v1/search?q=${encodeURIComponent(token)}`);
  expect(outsiderGlobal.some((item) => item.title.includes(token))).toBe(false);
});
