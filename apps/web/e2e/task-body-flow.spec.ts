import { expect, test, type Page } from "@playwright/test";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tbf",
  workspaceName: "Task Body Flow",
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

type TaskJson = { id: string; contentJson: unknown; archivedAt: string | null };

async function taskJson(page: Page, wsId: string, taskId: string): Promise<TaskJson> {
  const res = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
  expect(res.ok()).toBe(true);
  return res.json();
}

function firstBlockId(content: unknown): string | null {
  const doc = content as { content?: { attrs?: { id?: unknown } }[] };
  const id = doc.content?.[0]?.attrs?.id;
  return typeof id === "string" ? id : null;
}

test("task body is a collaborative room with revisions, block patch and restore", async ({
  page,
}) => {
  await ensureSetup(page);
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  const wsId = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  ).id as string;

  const projectRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key: "TBF", name: "Task Body", visibility: "workspace" },
  });
  expect(projectRes.status()).toBe(201);
  const project = (await projectRes.json()) as { id: string };
  const taskRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/tasks`,
    { data: { title: "본문 있는 태스크" } },
  );
  expect(taskRes.status()).toBe(201);
  const task = (await taskRes.json()) as { id: string; number: number };

  // 1. The task detail page joins `${ws}:task:${id}` and edits the body.
  await page.goto(`/w/${admin.workspaceSlug}/TBF-${task.number}`);
  const body = page.getByTestId("task-body");
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  const editor = body.locator(".fvoci-editor .ProseMirror");
  await expect(editor).toBeVisible();
  await expect(editor).toHaveAttribute("contenteditable", "true");
  await editor.click();
  await page.keyboard.type("첫 본문 버전");
  await body.getByRole("button", { name: "저장", exact: true }).click();
  await expect(body.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });
  await expect
    .poll(async () => JSON.stringify((await taskJson(page, wsId, task.id)).contentJson))
    .toContain("첫 본문 버전");

  // 2. Save a revision from the version panel.
  await body.getByTestId("revision-history").click();
  await body.getByTestId("revision-save").click();
  await expect(body.getByTestId("revision-item")).toHaveCount(1);

  // 3. A block patch through the API reaches the open editor live.
  const blockId = firstBlockId((await taskJson(page, wsId, task.id)).contentJson);
  expect(blockId).toBeTruthy();
  const patch = await page.request.patch(
    `/api/v1/workspaces/${wsId}/tasks/${task.id}/blocks/${blockId}`,
    { data: { type: "paragraph", content: [{ type: "text", text: "API로 바꾼 문단" }] } },
  );
  expect(patch.status(), await patch.text()).toBe(200);
  await expect(editor).toContainText("API로 바꾼 문단");
  await expect(editor).not.toContainText("첫 본문 버전");

  // 4. Restore the saved revision through the room.
  await body.getByTestId("revision-restore").first().click();
  await page.getByTestId("revision-restore-confirm").click();
  await expect(editor).toContainText("첫 본문 버전", { timeout: 15_000 });
  await expect
    .poll(async () => JSON.stringify((await taskJson(page, wsId, task.id)).contentJson))
    .toContain("첫 본문 버전");

  // 5. Reload: the body comes back from the persisted room.
  await page.reload();
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(editor).toContainText("첫 본문 버전");

  // 6. Archived task: the editor is read-only and a block patch is refused.
  const archive = await page.request.patch(`/api/v1/workspaces/${wsId}/tasks/${task.id}`, {
    data: { archived: true },
  });
  expect(archive.ok(), await archive.text()).toBe(true);
  await page.reload();
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(editor).toHaveAttribute("contenteditable", "false");
  const refused = await page.request.patch(
    `/api/v1/workspaces/${wsId}/tasks/${task.id}/blocks/${blockId}`,
    { data: { type: "paragraph" } },
  );
  expect(refused.status()).toBe(409);
  expect((await refused.json()).code).toBe("task_archived");
});
