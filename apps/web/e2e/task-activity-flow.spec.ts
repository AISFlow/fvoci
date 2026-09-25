import { expect, test, type Page } from "@playwright/test";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "taf",
  workspaceName: "Task Activity UI",
};

async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
}

test("태스크 활동: 변경과 댓글을 한 흐름에서 필터하고 다시 열어도 보존한다", async ({
  page,
}) => {
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

  const workspaceIdValue = await workspaceId(page, owner.workspaceSlug);
  const base = `/api/v1/workspaces/${workspaceIdValue}`;
  const projectResponse = await page.request.post(`${base}/projects`, {
    data: { key: "ACT", name: "태스크 활동 검증", visibility: "workspace" },
  });
  expect(projectResponse.ok()).toBeTruthy();
  const project: { id: string } = await projectResponse.json();
  const taskResponse = await page.request.post(
    `${base}/projects/${project.id}/tasks`,
    { data: { title: "배양 조건 확인", priority: "medium" } },
  );
  expect(taskResponse.ok()).toBeTruthy();
  const task: { id: string; number: number } = await taskResponse.json();
  const taskApi = `${base}/tasks/${task.id}`;
  const rootBody = "A/B 조건 결과를 정리했습니다.";
  const replyBody = "B 조건 재현성도 확인했습니다.";

  const rootCommentResponse = await page.request.post(`${taskApi}/comments`, {
    data: { body: rootBody },
  });
  expect(rootCommentResponse.ok()).toBeTruthy();
  const rootComment: { id: string } = await rootCommentResponse.json();
  const replyResponse = await page.request.post(`${taskApi}/comments`, {
    data: { body: replyBody, parentId: rootComment.id },
  });
  expect(replyResponse.ok()).toBeTruthy();

  const changedTitle = "배양 조건과 재현성 확인";
  const patchResponse = await page.request.patch(taskApi, {
    data: { title: changedTitle, priority: "high" },
  });
  expect(patchResponse.ok()).toBeTruthy();
  for (const archived of [true, false]) {
    const response = await page.request.patch(taskApi, { data: { archived } });
    expect(response.ok()).toBeTruthy();
  }

  const taskUrl = `/w/${owner.workspaceSlug}/ACT-${task.number}`;
  await page.goto(taskUrl);
  const activity = page.locator("#fv-comments");
  await expect(activity.getByRole("heading", { name: "활동" })).toBeVisible();
  await expect(activity.getByText(replyBody, { exact: true })).toBeVisible();
  await expect(
    activity.getByText("태스크를 생성했습니다.", { exact: false }),
  ).toBeVisible();
  await expect(activity.getByText("보관 상태", { exact: true })).toHaveCount(2);
  await expect(activity.getByText("보통", { exact: true })).toBeVisible();
  await expect(activity.getByText("높음", { exact: true })).toBeVisible();

  const rootDraft = activity.locator("[data-comment-compose] textarea");
  await rootDraft.fill("작성 중인 최상위 댓글");
  await activity.getByRole("button", { name: "답글" }).first().click();
  const replyDraft = activity.getByLabel("댓글을 입력하세요").first();
  await replyDraft.fill("작성 중인 답글");

  const filter = activity.getByLabel("활동 필터");
  await filter.selectOption("changes");
  await expect(activity.getByText(replyBody, { exact: true })).toHaveCount(0);
  await expect(activity.getByText("보관 상태", { exact: true })).toHaveCount(2);

  await filter.selectOption("comments");
  await expect(activity.getByText(replyBody, { exact: true })).toBeVisible();
  await expect(rootDraft).toHaveValue("작성 중인 최상위 댓글");
  await expect(activity.getByLabel("댓글을 입력하세요").first()).toHaveValue(
    "작성 중인 답글",
  );
  await expect(
    activity.getByText("태스크를 생성했습니다.", { exact: false }),
  ).toHaveCount(0);

  await page.reload();
  await expect(page.getByLabel("태스크 제목")).toHaveValue(changedTitle);
  await expect(
    page.locator("#fv-comments").getByText(replyBody, { exact: true }),
  ).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await activity.scrollIntoViewIfNeeded();
  expect(
    await activity.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
});
