import { expect, test } from "@playwright/test";
import { z } from "zod";
import { login } from "./helpers";

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
  const detailUrl = `/w/w5timer/READ-${task.number}`;
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
