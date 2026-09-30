import { z } from "zod";
import { execFileSync } from "node:child_process";
import { expect, test } from "@playwright/test";
import { readJson, flowSchemas, login } from "./helpers";

test.describe.configure({ mode: "serial" });
const owner = { email: "parity@example.com", password: "paritypass123" };
let workspaceId: string;
let project: {
  id: string;
  key: string;
  rootDocumentId: string | null;
};
const tasks: {
  id: string;
  title: string;
}[] = [];
test("workspace landing has authorized projects, counts, and eight due-ordered assigned rows", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("동등");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Parity workspace");
  await page.getByLabel("주소(영문)").fill("parity");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  const fixtureValue1 = (
    await readJson(await page.request.get("/api/v1/me/workspaces"), flowSchemas.workspaces)
  ).items[0];
  if (fixtureValue1 === undefined)
    throw new Error(
      'Missing fixture value: (\n    await readJson(await page.request.get("/api/v1/me/workspaces"), flowSchemas.workspaces)\n  ).items[0]',
    );
  workspaceId = fixtureValue1.id;
  const me = await readJson(await page.request.get("/api/v1/auth/me"), flowSchemas.user);
  expect(
    (
      await page.request.patch("/api/v1/auth/me", {
        data: { givenName: me.givenName, timezone: "Pacific/Honolulu" },
      })
    ).ok(),
  ).toBe(true);
  const created = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key: "META", name: "Metadata project", visibility: "workspace" },
  });
  expect(created.status()).toBe(201);
  project = await readJson(created, flowSchemas.project);
  const labels = [];
  for (let index = 0; index < 3; index++) {
    const response = await page.request.post(
      `/api/v1/workspaces/${workspaceId}/projects/${project.id}/labels`,
      { data: { name: `Label ${String(index)}`, color: "blue" } },
    );
    expect(response.status(), await response.text()).toBe(201);
    labels.push((await readJson(response, flowSchemas.id)).id);
  }
  // Reverse insertion order witnesses the actual server due-date ordering and limit.
  for (let index = 9; index >= 1; index--) {
    const response = await page.request.post(
      `/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`,
      {
        data: {
          title: `Due ${String(index)}`,
          dueDate: `2026-10-${String(index).padStart(2, "0")}`,
          type: "bug",
          priority: "high",
        },
      },
    );
    expect(response.status()).toBe(201);
    const task = await readJson(response, flowSchemas.item);
    tasks.push(task);
    expect(
      (
        await page.request.patch(`/api/v1/workspaces/${workspaceId}/tasks/${task.id}`, {
          data: { assigneeIds: [me.userId], labelIds: labels },
        })
      ).ok(),
    ).toBe(true);
  }
  const archived = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
    data: { key: "OLD", name: "Archived project", visibility: "workspace" },
  });
  expect(archived.status()).toBe(201);
  const old = await readJson(archived, flowSchemas.project);
  expect(
    (await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${old.id}/archive`)).ok(),
  ).toBe(true);
  const preview = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return (
      url.pathname === `/api/v1/workspaces/${workspaceId}/tasks` &&
      url.searchParams.get("limit") === "8"
    );
  });
  await page.goto("/w/parity");
  expect((await preview).ok()).toBe(true);
  const rows = page.getByTestId("workspace-assigned").locator("a.task-row");
  await expect(rows).toHaveCount(8);
  await expect(rows.first()).toContainText("Due 1");
  await expect(rows.last()).toContainText("Due 8");
  await expect(rows.first()).toContainText("10. 1.");
  await expect(rows.first()).toContainText("버그");
  await expect(rows.first()).toContainText("높음");
  await expect(rows.first()).toContainText("Label 0");
  await expect(rows.first()).toContainText("Label 1");
  await expect(rows.first()).toContainText("+1");
  await expect(rows.first().getByTitle("김동등")).toBeVisible();
  const counts = await readJson(
    await page.request.get("/api/v1/me/workspaces"),
    flowSchemas.workspaces,
  );
  const fixtureValue2 = counts.items[0];
  if (fixtureValue2 === undefined) throw new Error("Missing fixture value: counts.items[0]");
  await expect(page.getByTestId("workspace-totals")).toContainText(
    `문서 ${String(fixtureValue2.documentCount)}개`,
  );
  await expect(page.getByTestId("workspace-totals")).toContainText("프로젝트 2개");
  await expect(page.getByRole("link", { name: /OLD Archived project/ })).toBeVisible();
  await page.reload();
  await expect(rows).toHaveCount(8);
});

test("full my-tasks keeps metadata and navigation after reload", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await page.goto("/w/parity/my-tasks");
  const list = page.getByTestId("my-tasks");
  await expect(list.locator("a.task-row")).toHaveCount(9);
  const required1 = tasks.find((task) => task.title === "Due 1");
  if (required1 === undefined) {
    throw new Error('Missing fixture value: tasks.find((task) => task.title === "Due 1")');
  }
  const task = required1;
  const row = list.getByTestId(`my-task-${task.id}`);
  await expect(row).toContainText("Label 0");
  await expect(row).toContainText("높음");
  await expect(row.getByTitle("김동등")).toBeVisible();
  await page.reload();
  await expect(row).toContainText("버그");
  await row.click();
  await expect(page.getByRole("heading", { name: "Due 1", exact: true })).toBeVisible();
});

test("search draft commits after 300ms and project scope can clear and restore", async ({
  page,
}) => {
  await login(page, owner.email, owner.password);
  await page.goto(`/w/parity/search?q=old&tab=task&projectId=${project.id}`);
  const input = page.locator("#workspace-search-q");
  await expect(input).toBeVisible();
  const start = Date.now();
  await page.clock.install({ time: start });
  await page.clock.pauseAt(start + 1000);
  await input.fill("Due");
  await page.clock.runFor(299);
  await expect(page).toHaveURL(/q=old/);
  await page.clock.runFor(1);
  await expect(page).toHaveURL(/q=Due/);
  await expect(page.getByRole("radio", { name: /이 프로젝트만/ })).toBeChecked();
  await page.getByRole("radio", { name: "워크스페이스 전체" }).check();
  expect(new URL(page.url()).searchParams.has("projectId")).toBe(false);
  await expect(page.getByRole("radio", { name: /이 프로젝트만/ })).toBeVisible();
  await page.getByRole("radio", { name: /이 프로젝트만/ }).check();
  await expect(page).toHaveURL(new RegExp(`projectId=${project.id}`));
  await page.reload();
  await expect(input).toHaveValue("Due");
  await expect(page.getByRole("radio", { name: /이 프로젝트만/ })).toBeChecked();
});

test("trash timestamp follows the saved user zone instead of the browser zone", async ({
  page,
}) => {
  await login(page, owner.email, owner.password);
  const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, {
    data: { title: "Zone trash", parentId: null },
  });
  expect(response.status()).toBe(201);
  const doc = await readJson(response, flowSchemas.document);
  expect(
    (await page.request.post(`/api/v1/workspaces/${workspaceId}/documents/${doc.id}/trash`)).ok(),
  ).toBe(true);
  await page.goto("/w/parity/trash");
  const row = page.locator(".trash-page__row").filter({ hasText: "Zone trash" });
  const time = row.locator("time");
  await expect(time).toBeVisible();
  const iso = await time.getAttribute("datetime");
  const expected = await page.evaluate((value) => {
    const required2 = value;
    if (required2 === null) {
      throw new Error("Missing fixture value: value");
    }
    return new Date(required2).toLocaleString("ko-KR", {
      hour12: false,
      timeZone: "Pacific/Honolulu",
      year: "numeric",
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }, iso);
  await expect(time).toHaveText(expected);
  await page.reload();
  await expect(time).toHaveText(expected);
});

test("wiki tag URLs include child-only matches and project documents; unfiltered drag moves and sorts persist", async ({
  page,
}) => {
  await login(page, owner.email, owner.password);
  const wiki: {
    id: string;
    displayId: string;
  }[] = [];
  for (const title of ["Wiki parent", "Wiki second", "Wiki third"]) {
    const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, {
      data: { title, parentId: null },
    });
    expect(response.status()).toBe(201);
    wiki.push(await readJson(response, flowSchemas.createdDocument));
  }
  const fixtureValue3 = wiki[0];
  if (fixtureValue3 === undefined) throw new Error("Missing fixture value: wiki[0]");
  const childResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, {
    data: { title: "Tagged child", parentId: fixtureValue3.id },
  });
  expect(childResponse.status()).toBe(201);
  const child = await readJson(childResponse, flowSchemas.document);
  const projectResponse = await page.request.post(
    `/api/v1/workspaces/${workspaceId}/projects/${project.id}/documents`,
    { data: { title: "Tagged project child", parentId: project.rootDocumentId } },
  );
  expect(projectResponse.status()).toBe(201);
  const projectChild = await readJson(projectResponse, flowSchemas.document);
  const tagResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/document-tags`, {
    data: { name: "Planning", color: "gray" },
  });
  expect(tagResponse.status()).toBe(201);
  const tag = await readJson(tagResponse, flowSchemas.tag);
  for (const [doc, path] of [
    [child, `/api/v1/workspaces/${workspaceId}/documents/${child.id}/tags`],
    [
      projectChild,
      `/api/v1/workspaces/${workspaceId}/projects/${project.id}/documents/${projectChild.id}/tags`,
    ],
  ] as const) {
    expect((await page.request.post(path, { data: { tagId: tag.id } })).ok()).toBe(true);
    expect(doc.id).toBeTruthy();
  }
  await page.goto(`/w/parity/wiki?tag=${tag.id}`);
  const selected = page.getByRole("button", { name: "Planning", exact: true });
  await expect(selected).toHaveAttribute("aria-pressed", "true");
  const childLink = page.getByRole("link", { name: /Tagged child/ });
  await expect(childLink).toBeVisible();
  await expect(childLink).toHaveAttribute("draggable", "false");
  await expect(page.getByRole("link", { name: /Tagged project child/ })).toBeVisible();
  await expect(page.getByRole("link", { name: /Wiki parent/ })).toHaveCount(0);
  await page.reload();
  await expect(childLink).toBeVisible();
  await selected.click();
  await expect(page).toHaveURL(/\/w\/parity\/wiki$/);
  const fixtureValue4 = wiki[0];
  if (fixtureValue4 === undefined) throw new Error("Missing fixture value: wiki[0]");
  const parent = page.getByTestId(`wiki-doc-${fixtureValue4.displayId}`);
  const fixtureValue5 = wiki[1];
  if (fixtureValue5 === undefined) throw new Error("Missing fixture value: wiki[1]");
  const second = page.getByTestId(`wiki-doc-${fixtureValue5.displayId}`);
  const fixtureValue6 = wiki[2];
  if (fixtureValue6 === undefined) throw new Error("Missing fixture value: wiki[2]");
  const third = page.getByTestId(`wiki-doc-${fixtureValue6.displayId}`);
  const sorted = page.waitForResponse((response) => {
    const fixtureValue7 = wiki[2];
    if (fixtureValue7 === undefined) throw new Error("Missing fixture value: wiki[2]");
    return (
      new URL(response.url()).pathname ===
        `/api/v1/workspaces/${workspaceId}/documents/${fixtureValue7.id}/sort` &&
      response.request().method() === "POST"
    );
  });
  await third.dragTo(parent, { targetPosition: { x: 25, y: 1 } });
  expect((await sorted).ok()).toBe(true);
  const roots = page.locator(".wiki-home__section > ul > li > a");
  await expect(roots.first()).toContainText("Wiki third");
  const moved = page.waitForResponse((response) => {
    const fixtureValue8 = wiki[1];
    if (fixtureValue8 === undefined) throw new Error("Missing fixture value: wiki[1]");
    return (
      new URL(response.url()).pathname ===
        `/api/v1/workspaces/${workspaceId}/documents/${fixtureValue8.id}/move` &&
      response.request().method() === "POST"
    );
  });
  await second.dragTo(parent);
  expect((await moved).ok()).toBe(true);
  const parentBranch = parent.locator("..");
  await expect(parentBranch.getByRole("link", { name: /Wiki second/ })).toBeVisible();
  await page.reload();
  await expect(roots.first()).toContainText("Wiki third");
  await expect(parentBranch.getByRole("link", { name: /Wiki second/ })).toBeVisible();
  const fixtureValue9 = wiki[1];
  if (fixtureValue9 === undefined) throw new Error("Missing fixture value: wiki[1]");
  const fixtureValue10 = wiki[0];
  if (fixtureValue10 === undefined) throw new Error("Missing fixture value: wiki[0]");
  expect(
    (
      await readJson(
        await page.request.get(`/api/v1/workspaces/${workspaceId}/documents/${fixtureValue9.id}`),
        flowSchemas.document,
      )
    ).parentId,
  ).toBe(fixtureValue10.id);
  await page.goto("/w/parity/wiki?tag=malformed");
  await expect(page.getByRole("alert")).toBeVisible();
});

test("source tag:name search filters documents and leaves task hits in the real API", async ({
  page,
}, testInfo) => {
  await login(page, owner.email, owner.password);
  const tag = (
    await readJson(
      await page.request.get(`/api/v1/workspaces/${workspaceId}/document-tags`),
      flowSchemas.tags,
    )
  ).items.find((tag: { name: string }) => tag.name === "Planning");
  if (tag === undefined) throw new Error("Missing fixture value: tag");
  expect(tag).toBeTruthy();
  // Complete only the already committed setup prefix through the actual
  // search worker before starting the fresh task's unchanged recall window.
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  const adminUrl = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  if (!container || !adminUrl) {
    throw new Error("isolated PostgreSQL fixture context is required");
  }
  const fixtureContainer = container;
  const fixtureDatabase = new URL(adminUrl).pathname.slice(1);
  expect(workspaceId).toMatch(/^[0-9a-f-]{36}$/i);
  function fixtureJson<T>(sql: string, schema: z.ZodType<T>): T {
    const output = execFileSync(
      "docker",
      [
        "exec",
        "-i",
        fixtureContainer,
        "psql",
        "-U",
        "postgres",
        "-d",
        fixtureDatabase,
        "-qAt",
        "-v",
        "ON_ERROR_STOP=1",
      ],
      {
        input: `BEGIN READ ONLY;\n${sql}\nCOMMIT;\n`,
        encoding: "utf8",
        timeout: 5000,
        stdio: ["pipe", "pipe", "pipe"],
      },
    );
    return schema.parse(JSON.parse(output.trim()));
  }
  const watermark = fixtureJson(
    `
    SELECT row_to_json(w) FROM (
      SELECT id, xact::text AS xact, seq::text AS seq FROM fvoci.events
      WHERE workspace_id = '${workspaceId}'::uuid ORDER BY xact DESC, seq DESC LIMIT 1
    ) w;
  `,
    z.object({ id: z.string(), xact: z.string(), seq: z.string() }),
  );
  expect(watermark.id).toMatch(/^[0-9a-f-]{36}$/i);
  expect(watermark.xact).toMatch(/^\d+$/);
  expect(watermark.seq).toMatch(/^\d+$/);
  const setupStarted = Date.now();
  const prefixSamples: {
    elapsedMs: number;
    total: number;
    pending: number;
    failures: number;
  }[] = [];
  try {
    await expect
      .poll(
        () => {
          const state = fixtureJson(
            `
        WITH prior AS (
          SELECT id FROM fvoci.events WHERE workspace_id = '${workspaceId}'::uuid
            AND (xact, seq) <= ('${watermark.xact}'::xid8, ${watermark.seq}::bigint)
        )
        SELECT json_build_object(
          'total', (SELECT count(*) FROM prior),
          'pending', (SELECT count(*) FROM prior e WHERE NOT EXISTS (
            SELECT 1 FROM fvoci.processed_events p WHERE p.consumer = 'search-index' AND p.event_id = e.id
          )),
          'failures', (SELECT count(*) FROM prior e JOIN fvoci.outbox_failures f ON f.event_id = e.id WHERE f.consumer = 'search-index')
        );
      `,
            z.object({ total: z.number(), pending: z.number(), failures: z.number() }),
          );
          prefixSamples.push({ elapsedMs: Date.now() - setupStarted, ...state });
          expect(state.total).toBeGreaterThan(0);
          if (state.failures !== 0) {
            throw new Error("preceding search setup has failed or dead-letter events");
          }
          return state.pending;
        },
        { timeout: 15000 },
      )
      .toBe(0);
  } finally {
    await testInfo.attach("prior-search-prefix-readiness", {
      body: JSON.stringify({ workspaceId, watermark, prefixSamples }, null, 2),
      contentType: "application/json",
    });
  }
  // The preceding fixture writes many resources. Prove its real search
  // readiness before timing recall of the next task; do not mix the existing
  // outbox backlog with that task's five-second assertion.
  await expect
    .poll(async () => {
      const responses = await Promise.all([
        page.request.get(`/api/v1/workspaces/${workspaceId}/search?q=Tagged&type=document`),
        page.request.get(`/api/v1/workspaces/${workspaceId}/search?q=Due&type=task`),
      ]);
      for (const response of responses) expect(response.ok()).toBe(true);
      const [documents, priorTasks] = await Promise.all(
        responses.map((response) => readJson(response, flowSchemas.search)),
      );
      const fixtureValue11 = documents;
      if (fixtureValue11 === undefined) throw new Error("Missing fixture value: documents");
      const fixtureValue12 = priorTasks;
      if (fixtureValue12 === undefined) throw new Error("Missing fixture value: priorTasks");
      return {
        documents: fixtureValue11.items.map((item: { title: string }) => item.title).sort(),
        tasks: fixtureValue12.items.map((item: { id: string }) => item.id).sort(),
      };
    })
    .toEqual({
      documents: ["Tagged child", "Tagged project child"],
      tasks: tasks.map((task) => task.id).sort(),
    });
  const created = await page.request.post(
    `/api/v1/workspaces/${workspaceId}/projects/${project.id}/tasks`,
    { data: { title: "Tagged task" } },
  );
  expect(created.status()).toBe(201);
  const samples: unknown[] = [];
  try {
    await expect
      .poll(async () => {
        const responses = await Promise.all([
          page.request.get(
            `/api/v1/workspaces/${workspaceId}/search?q=Tagged&type=all&tag=${tag.id}`,
          ),
          page.request.get(`/api/v1/workspaces/${workspaceId}/search?q=Tagged&type=all`),
          page.request.get(`/api/v1/workspaces/${workspaceId}/search?q=Tagged&type=task`),
        ]);
        for (const response of responses) expect(response.ok()).toBe(true);
        const [tagged, untagged, taskOnly] = await Promise.all(
          responses.map((response) => readJson(response, flowSchemas.search)),
        );
        samples.push({ at: new Date().toISOString(), tagged, untagged, taskOnly });
        const fixtureValue13 = tagged;
        if (fixtureValue13 === undefined) throw new Error("Missing fixture value: tagged");
        return fixtureValue13.items.map((item: { title: string }) => item.title).sort();
      })
      .toEqual(["Tagged child", "Tagged project child", "Tagged task"]);
  } finally {
    await testInfo.attach("real-tag-and-untagged-recall", {
      body: JSON.stringify(
        {
          createdTask: await readJson(created, flowSchemas.item),
          workspaceId,
          project,
          tag,
          samples,
        },
        null,
        2,
      ),
      contentType: "application/json",
    });
  }
  await page.goto("/w/parity/search?q=tag%3Aplanning%20Tagged");
  await expect(page.getByRole("link", { name: /Tagged child/ })).toBeVisible();
  await expect(page.getByRole("link", { name: /Tagged task/ })).toBeVisible();
  await page.getByRole("tab", { name: "댓글", exact: true }).click();
  await expect(page.getByText("결과가 없습니다", { exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "태스크", exact: true }).click();
  await expect(page.getByRole("link", { name: /Tagged task/ })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("link", { name: /Tagged task/ })).toBeVisible();
});
