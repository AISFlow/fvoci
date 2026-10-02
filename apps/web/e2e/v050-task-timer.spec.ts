import { expect, test } from "@playwright/test";
import { z } from "zod";
import { createE2eUser, login } from "./helpers";

const credentials = { email: "timer@example.com", password: "supersecret1" };
const workspaces = z.object({ items: z.array(z.object({ id: z.string(), slug: z.string() })) });
const taskShape = z.object({ id: z.string(), number: z.number(), statusId: z.string() });
const timerShape = z.object({
  run: z
    .object({
      id: z.string(),
      status: z.enum(["running", "paused", "stopped"]),
      version: z.number(),
      runningSince: z.string().nullable(),
      elapsedMilliseconds: z.number(),
    })
    .nullable(),
  actualMilliseconds: z.number(),
});

test("detail start commits server intervals; a new MyTasks client pauses, resumes and stops the same task", async ({
  page,
  browser,
}) => {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true }))
      .or(page.getByRole("button", { name: "로그아웃" })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("연구");
    await page.getByLabel("이메일").fill(credentials.email);
    await page.getByLabel("비밀번호").fill(credentials.password);
    await page.getByLabel("워크스페이스 이름").fill("읽기 연구 계획");
    await page.getByLabel("주소(영문)").fill("w5timer");
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else await login(page, credentials.email, credentials.password);
  const workspaceResponse = await page.request.get("/api/v1/me/workspaces");
  expect(workspaceResponse.ok()).toBe(true);
  const workspace = workspaces
    .parse(await workspaceResponse.json())
    .items.find((row) => row.slug === "w5timer");
  expect(workspace).toBeTruthy();
  if (!workspace) throw new Error("missing timer fixture workspace");
  const me = z
    .object({ userId: z.string() })
    .parse(await (await page.request.get("/api/v1/auth/me")).json());
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
    data: { key: "READ", name: "연구 목표와 자료", visibility: "workspace" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await projectResponse.json());
  const taskResponse = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/projects/${project.id}/tasks`,
    { data: { title: "한글 자료 읽기와 연결된 연구 노트" } },
  );
  expect(taskResponse.status(), await taskResponse.text()).toBe(201);
  const task = taskShape.parse(await taskResponse.json());
  const assigned = await page.request.patch(`/api/v1/workspaces/${workspace.id}/tasks/${task.id}`, {
    data: { assigneeIds: [me.userId], estimate: "30" },
  });
  expect(assigned.ok(), await assigned.text()).toBe(true);
  const detailUrl = `/w/w5timer/READ-${String(task.number)}`;
  const apiUrl = `/api/v1/workspaces/${workspace.id}/tasks/${task.id}/timer`;
  await page.goto(detailUrl);
  const detail = page.getByTestId(`task-stopwatch-${task.id}`);
  await expect(detail.getByTestId("timer-start")).toBeEnabled();
  await detail.getByLabel("측정 메모").fill("읽기 구간");
  await detail.getByTestId("timer-start").click();
  await expect(detail.getByTestId("timer-state")).toHaveText("측정 중");
  const committed = timerShape.parse(await (await page.request.get(apiUrl)).json());
  expect(committed.run?.status).toBe("running");
  // Fresh browser context/session, not this page's optimistic state/cache.
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const fresh = await context.newPage();
    await login(fresh, credentials.email, credentials.password);
    await fresh.goto("/w/w5timer/my-tasks");
    const mine = fresh.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mine.getByTestId("timer-state")).toHaveText("측정 중");
    expect(timerShape.parse(await (await fresh.request.get(apiUrl)).json()).run?.id).toBe(
      committed.run?.id,
    );
    await mine.getByTestId("timer-pause").click();
    await expect(mine.getByTestId("timer-state")).toHaveText("일시정지");
    const paused = timerShape.parse(await (await fresh.request.get(apiUrl)).json());
    expect(paused.run?.runningSince).toBeNull();
    await fresh.reload();
    await expect(mine.getByTestId("timer-state")).toHaveText("일시정지");
    const stillPaused = timerShape.parse(await (await fresh.request.get(apiUrl)).json());
    expect(stillPaused.run?.elapsedMilliseconds).toBe(paused.run?.elapsedMilliseconds);
    await mine.getByTestId("timer-resume").click();
    await expect(mine.getByTestId("timer-state")).toHaveText("측정 중");
    await mine.getByTestId("timer-stop").click();
    await expect(mine.getByTestId("timer-start")).toBeEnabled();
    expect(timerShape.parse(await (await fresh.request.get(apiUrl)).json()).run).toBeNull();
    const after = taskShape.parse(
      await (await fresh.request.get(`/api/v1/workspaces/${workspace.id}/tasks/${task.id}`)).json(),
    );
    expect(after.statusId).toBe(task.statusId);
    await page.reload();
    await expect(detail.getByTestId("timer-start")).toBeEnabled();
    await expect(detail.getByTestId("timer-actual")).toHaveText(
      await mine.getByTestId("timer-actual").innerText(),
    );
  } finally {
    await context.close();
  }
});

async function timerWorkspace(page: import("@playwright/test").Page): Promise<string> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true }))
      .or(page.getByRole("button", { name: "로그아웃" })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("연구");
    await page.getByLabel("이메일").fill(credentials.email);
    await page.getByLabel("비밀번호").fill(credentials.password);
    await page.getByLabel("워크스페이스 이름").fill("읽기 연구 계획");
    await page.getByLabel("주소(영문)").fill("w5timer");
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else await login(page, credentials.email, credentials.password);
  const workspaceResponse = await page.request.get("/api/v1/me/workspaces");
  expect(workspaceResponse.ok()).toBe(true);
  const workspace = workspaces
    .parse(await workspaceResponse.json())
    .items.find((row) => row.slug === "w5timer");
  expect(workspace).toBeTruthy();
  if (!workspace) throw new Error("missing timer fixture workspace");
  return workspace.id;
}

async function permissionFixture(
  page: import("@playwright/test").Page,
  key: string,
  role: "member" | "viewer",
) {
  const workspaceId = await timerWorkspace(page);
  const email = `${key.toLowerCase()}@example.com`;
  createE2eUser(email, credentials.password, "권한 검사", {
    workspaceSlug: "w5timer",
    membershipRole: "member",
  });
  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key, name: "권한 회수 검사", visibility: "private" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = z.object({ id: z.string() }).parse(await projectResponse.json());
  return { workspaceId, email, project, role };
}

test("authoritative revoked timer GET retires mounted private state while 503 preserves the server anchor", async ({
  page,
  browser,
}) => {
  const fixture = await permissionFixture(page, "TACL", "member");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const editor = await context.newPage();
    await login(editor, fixture.email, credentials.password);
    const me = z
      .object({ userId: z.string() })
      .parse(await (await editor.request.get("/api/v1/auth/me")).json());
    const memberUrl = `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`;
    const grant = await page.request.post(memberUrl, {
      data: { userId: me.userId, role: fixture.role },
    });
    expect(grant.ok(), await grant.text()).toBe(true);
    const taskResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      { data: { title: "권한 회수 뒤 사적인 측정" } },
    );
    expect(taskResponse.status(), await taskResponse.text()).toBe(201);
    const task = taskShape.parse(await taskResponse.json());
    const assign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}`,
      { data: { assigneeIds: [me.userId] } },
    );
    expect(assign.ok(), await assign.text()).toBe(true);
    const timerUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/timer`;
    await editor.goto("/w/w5timer/my-tasks");
    const mounted = editor.getByTestId(`task-stopwatch-${task.id}`);
    await expect(mounted.getByTestId("timer-start")).toBeEnabled();
    await mounted.getByTestId("timer-start").click();
    await expect(mounted.getByTestId("timer-state")).toHaveText("측정 중");
    const committed = timerShape.parse(await (await editor.request.get(timerUrl)).json());
    expect(committed.run?.id).toBeTruthy();
    await editor.route(`**${timerUrl}`, async (route) => {
      if (route.request().method() !== "GET") return route.continue();
      await route.fulfill({
        status: 503,
        contentType: "application/problem+json",
        body: JSON.stringify({ type: "about:blank", title: "일시적인 연결 실패", status: 503 }),
      });
    });
    await expect(mounted.getByRole("alert")).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveText("측정 중");
    await expect(mounted.getByTestId("timer-actual")).toBeVisible();
    // Only the unrelated list transport is held to keep the original consumer
    // mounted. The timer denial below comes from real Rust/current project ACL.
    await editor.route(
      (url) => url.pathname === `/api/v1/workspaces/${fixture.workspaceId}/tasks`,
      async (route) => {
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "목록 연결 실패", status: 503 }),
        });
      },
    );
    await editor.unroute(`**${timerUrl}`);
    const realDenial = editor.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === timerUrl &&
        response.request().method() === "GET" &&
        response.status() === 404,
    );
    const revoke = await page.request.delete(`${memberUrl}/${me.userId}`);
    expect(revoke.ok(), await revoke.text()).toBe(true);
    const denial = await realDenial;
    expect((await denial.text()).includes("권한 회수 뒤 사적인 측정")).toBe(false);
    await expect(mounted).toBeVisible();
    await expect(mounted.getByTestId("timer-state")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-actual")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-elapsed")).toHaveCount(0);
    await expect(mounted.getByTestId("timer-start")).toBeDisabled();
    const actualDenied = await editor.request.get(timerUrl);
    expect(actualDenied.status()).toBe(404);
    const membership = await editor.request.get("/api/v1/me/workspaces");
    expect(
      workspaces.parse(await membership.json()).items.some((row) => row.id === fixture.workspaceId),
    ).toBe(true);
  } finally {
    await context.close();
  }
});

test("MyTasks timer controls follow actual View Edit archive and demotion capability", async ({
  page,
  browser,
}) => {
  const fixture = await permissionFixture(page, "TCAP", "viewer");
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
  try {
    const viewer = await context.newPage();
    await login(viewer, fixture.email, credentials.password);
    const me = z
      .object({ userId: z.string() })
      .parse(await (await viewer.request.get("/api/v1/auth/me")).json());
    const membersUrl = `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/members`;
    const grant = await page.request.post(membersUrl, {
      data: { userId: me.userId, role: "viewer" },
    });
    expect(grant.ok(), await grant.text()).toBe(true);
    const taskResponse = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/tasks`,
      { data: { title: "읽기 권한으로 보는 내 작업" } },
    );
    expect(taskResponse.status(), await taskResponse.text()).toBe(201);
    const task = taskShape.parse(await taskResponse.json());
    const assign = await page.request.patch(
      `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}`,
      { data: { assigneeIds: [me.userId] } },
    );
    expect(assign.ok(), await assign.text()).toBe(true);
    const entriesUrl = `/api/v1/workspaces/${fixture.workspaceId}/tasks/${task.id}/time-entries`;
    const capability = async () =>
      z
        .object({ canCreate: z.boolean() })
        .parse(await (await viewer.request.get(entriesUrl)).json()).canCreate;
    expect(await capability()).toBe(false);
    await viewer.goto("/w/w5timer/my-tasks");
    const timer = viewer.getByTestId(`task-stopwatch-${task.id}`);
    await expect(timer).toBeVisible();
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
    const promote = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "member" },
    });
    expect(promote.ok(), await promote.text()).toBe(true);
    expect(await capability()).toBe(true);
    await expect(timer.getByTestId("timer-start")).toBeEnabled();
    const demote = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "viewer" },
    });
    expect(demote.ok(), await demote.text()).toBe(true);
    expect(await capability()).toBe(false);
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
    const again = await page.request.patch(`${membersUrl}/${me.userId}`, {
      data: { role: "member" },
    });
    expect(again.ok(), await again.text()).toBe(true);
    await expect(timer.getByTestId("timer-start")).toBeEnabled();
    const archive = await page.request.post(
      `/api/v1/workspaces/${fixture.workspaceId}/projects/${fixture.project.id}/archive`,
    );
    expect(archive.ok(), await archive.text()).toBe(true);
    expect(await capability()).toBe(false);
    await expect(timer.getByTestId("timer-start")).toBeDisabled();
  } finally {
    await context.close();
  }
});
