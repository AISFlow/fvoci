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

const member = {
  email: "notif-member@example.com",
  password: "memberpass1",
  givenName: "알림",
  familyName: "수신",
};

test("assignment shows unread badge, inbox, and mark-read", async ({ page }) => {
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

  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspace = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === owner.workspaceSlug,
  );
  expect(workspace).toBeTruthy();
  const membersRes = await page.request.get(`/api/v1/workspaces/${workspace.id}/members`);
  expect(membersRes.ok()).toBe(true);
    let memberId = (await membersRes.json()).items.find(
      (item: { email: string }) => item.email.toLowerCase() === member.email.toLowerCase(),
    )?.userId;
  expect(memberId).toBeTruthy();

  const projRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
    data: { key: "NTF", name: "알림", visibility: "workspace" },
  });
  expect(projRes.status()).toBe(201);
  const project = await projRes.json();
  const projectId = project.id;
  const taskRes = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/projects/${projectId}/tasks`,
    { data: { title: "알림 수신 확인 태스크" } },
  );
  expect(taskRes.status()).toBe(201);
  const task = await taskRes.json();
  const assignRes = await page.request.patch(`/api/v1/workspaces/${workspace.id}/tasks/${task.id}`, {
    data: { assigneeIds: [memberId] },
  });
  expect(assignRes.status()).toBe(200);

  const groupRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/groups`, {
    data: { name: "알림팀" },
  });
  expect(groupRes.status(), await groupRes.text()).toBe(201);
  const groupId = (await groupRes.json()).id;
  const addRes = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/groups/${groupId}/members`,
    { data: { userId: memberId } },
  );
  expect(addRes.status(), await addRes.text()).toBe(201);
  const docCommentRes = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/projects/${projectId}/documents/${project.rootDocumentId}/comments`,
    { data: { body: "@알림팀 확인", mentionedGroupIds: [groupId] } },
  );
  expect(docCommentRes.status(), await docCommentRes.text()).toBe(201);

  await logout(page);
  await login(page, member.email, member.password);
  await page.goto(`/w/${owner.workspaceSlug}/wiki`);
  // Both notifications (assignment, group mention) are delivered by the
  // outbox relay; wait until both exist before loading the inbox, which does
  // not refetch on its own.
  await expect
    .poll(
      async () => {
        const res = await page.request.get(
          `/api/v1/workspaces/${workspace.id}/notifications`,
        );
        if (!res.ok()) return false;
        const items: { verb: string }[] = (await res.json()).items;
        return items.some((item) => item.verb.startsWith("comment."));
      },
      { timeout: 15_000 },
    )
    .toBe(true);
  await expect(
    page.getByRole("button", { name: /안 읽은 알림 \d+건/ }),
  ).toBeVisible({ timeout: 15_000 });

  await page.goto(`/w/${owner.workspaceSlug}/notifications`);
  const item = page.getByText(
    `태스크 #${task.number} 「알림 수신 확인 태스크」의 담당자로 지정되었습니다`,
    { exact: true },
  );
  await expect(item).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText("문서에 새 댓글이 달렸습니다", { exact: true })).toBeVisible();
  await item.click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/[A-Z0-9-]+-\\d+$`));

  await page.goto(`/w/${owner.workspaceSlug}/notifications`);
  const readAll = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/api/v1/workspaces/${workspace.id}/notifications/read-all`) &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "전체 읽음" }).click();
  expect((await readAll).status()).toBe(200);
  await page.getByRole("tab", { name: "안 읽음" }).click();
  await expect(page.getByText("알림이 없습니다")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("button", { name: "알림", exact: true })).toBeVisible({
    timeout: 15_000,
  });
});

