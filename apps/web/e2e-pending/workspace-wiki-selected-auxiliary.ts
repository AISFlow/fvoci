import { randomUUID } from "node:crypto";
import { expect, type Browser, type Page, type TestInfo } from "@playwright/test";
import type { components } from "../src/generated/api";
import { watchCspViolations } from "../e2e/helpers";
import {
  admin,
  closeCollabContext,
  login,
  member,
  newCollabContext,
  waitConnected,
} from "./collab-helpers";
import { SESSION_COOKIE, UUID_RE } from "./collab-wire";

type Schema = components["schemas"];
type AuxiliaryFixture = {
  tag: Schema["DocumentTagOutput"];
  comment: Schema["CommentOutput"];
  project: Schema["ProjectOutput"];
  task: Schema["TaskOutput"];
  taskDisplayId: string;
  originRequestId: string;
  sourceBlockId: string;
};

const auxiliarySuffixes = ["tags", "task-origins", "task-projects", "comments"] as const;

async function createAuxiliaryFixture(
  page: Page,
  workspaceId: string,
  document: Schema["DocumentMetaResponse"],
  creatorId: string,
  sourceBlockId: string,
): Promise<AuxiliaryFixture> {
  const workspacePath = `/api/v1/workspaces/${workspaceId}`;
  const documentPath = `${workspacePath}/documents/${document.id}`;
  // These are the maintained tags, comment-compose and origin API writers.
  // Never seed auxiliary rows through an owner DB connection or mock a GET.
  const tagResponse = await page.request.post(`${workspacePath}/document-tags`, {
    data: {
      name: "태그 中 😀",
      color: "violet",
    } satisfies Schema["DocumentTagCreateBody"],
  });
  expect(tagResponse.status()).toBe(201);
  const tag = (await tagResponse.json()) as Schema["DocumentTagOutput"];
  expect(tag.id).toMatch(UUID_RE);
  expect(tag).toMatchObject({ workspaceId, name: "태그 中 😀", color: "violet" });
  const assignment = await page.request.post(`${documentPath}/tags`, { data: { tagId: tag.id } });
  expect(assignment.status()).toBe(200);
  expect(await assignment.json()).toEqual(tag);

  const commentResponse = await page.request.post(`${documentPath}/comments`, {
    data: { body: "실제 댓글 😀", mentionedUserIds: [], mentionedGroupIds: [] },
  });
  expect(commentResponse.status()).toBe(201);
  const comment = (await commentResponse.json()) as Schema["CommentOutput"];
  expect(comment.id).toMatch(UUID_RE);
  expect(comment).toMatchObject({
    workspaceId,
    documentId: document.id,
    createdBy: creatorId,
    body: "실제 댓글 😀",
    parentId: null,
    taskId: null,
  });

  const projectResponse = await page.request.post(`${workspacePath}/projects`, {
    data: { key: "AUX", name: "실제 프로젝트", visibility: "workspace" },
  });
  expect(projectResponse.status()).toBe(201);
  const project = (await projectResponse.json()) as Schema["ProjectOutput"];
  expect(project.id).toMatch(UUID_RE);
  expect(project).toMatchObject({
    workspaceId,
    createdBy: creatorId,
    key: "AUX",
    name: "실제 프로젝트",
    visibility: "workspace",
  });
  // Reference the actual retained native wiki block without supplying body JSON.
  // The maintained service initializes the task and leaves source history intact.
  expect(sourceBlockId).toMatch(UUID_RE);
  const originRequestId = randomUUID();
  const taskResponse = await page.request.post(`${documentPath}/tasks`, {
    data: {
      projectId: project.id,
      requestId: originRequestId,
      anchor: sourceBlockId,
      task: { title: "실제 작업 😀" },
    } satisfies Schema["DocumentTaskCreateBody"],
  });
  expect(taskResponse.status()).toBe(201);
  const taskId = ((await taskResponse.json()) as Schema["DocumentTaskCreateOutput"]).taskId;
  expect(taskId).toMatch(UUID_RE);
  const taskRead = await page.request.get(`${workspacePath}/tasks/${taskId}`);
  expect(taskRead.status()).toBe(200);
  const task = (await taskRead.json()) as Schema["TaskOutput"];
  expect(task).toMatchObject({
    id: taskId,
    workspaceId,
    projectId: project.id,
    createdBy: creatorId,
    title: "실제 작업 😀",
  });
  expect(Number.isInteger(task.number)).toBe(true);
  expect(task.number).toBeGreaterThan(0);
  const taskDisplayId = `${project.key}-${String(task.number)}`;
  return { tag, comment, project, task, taskDisplayId, originRequestId, sourceBlockId };
}

async function observeMountedAuxiliary(
  page: Page,
  documentPath: string,
  document: Schema["DocumentMetaResponse"],
  fixture: AuxiliaryFixture,
) {
  // Only browser-page network responses qualify here. page.request GETs
  // cannot satisfy these waits; each new navigation must mount all four consumers.
  const pending = auxiliarySuffixes.map((suffix) =>
    page.waitForResponse(
      (response) =>
        response.request().method() === "GET" &&
        new URL(response.url()).pathname === `${documentPath}/${suffix}`,
    ),
  );
  await page.goto(`/w/${admin.workspaceSlug}/WIKI-${String(document.number)}`);
  const responses = await Promise.all(pending);
  for (const response of responses) expect(response.status(), response.url()).toBe(200);
  const tags = (await responses[0].json()) as Schema["DocumentTagListResponse"];
  const origins = (await responses[1].json()) as Schema["TaskOriginListResponse"];
  const projects = (await responses[2].json()) as Schema["TaskProjectPickerResponse"];
  const comments = (await responses[3].json()) as Schema["CommentListResponse"];
  expect(tags.items).toEqual([fixture.tag]);
  expect(comments.items).toEqual([fixture.comment]);
  expect(comments.nextCursor).toBeNull();
  expect(origins.count).toBe(1);
  expect(origins.nextCursor).toBeNull();
  expect(origins.items).toEqual([
    {
      taskId: fixture.task.id,
      taskTitle: fixture.task.title,
      taskDisplayId: fixture.taskDisplayId,
      documentId: document.id,
      documentTitle: document.title,
      documentDisplayId: `WIKI-${String(document.number)}`,
      anchor: fixture.sourceBlockId,
    },
  ]);
  expect(projects.items.length).toBeGreaterThan(0);
  expect(projects.items).toContainEqual({
    id: fixture.project.id,
    name: fixture.project.name,
    key: fixture.project.key,
    visibility: "workspace",
  });
  expect(projects.canCreateProject).toBe(true);
  expect(projects.suggestedId).toMatch(UUID_RE);
  expect(projects.items.some((project) => project.id === projects.suggestedId)).toBe(true);
  const tagsBar = page.getByTestId("document-tags-bar");
  await expect(tagsBar.locator(".tags-bar__item .tag-chip")).toHaveText(fixture.tag.name);
  await expect(tagsBar.locator(".tag-chip[data-color='violet']")).toBeVisible();
  const originsPanel = page.getByRole("region", { name: "연결 태스크" });
  await expect(originsPanel.getByRole("heading")).toHaveText("연결 태스크 (1)");
  await expect(
    originsPanel.getByRole("link", {
      name: `${fixture.taskDisplayId} · ${fixture.task.title}`,
      exact: true,
    }),
  ).toHaveAttribute("href", `/w/${admin.workspaceSlug}/${fixture.taskDisplayId}`);
  const option = originsPanel
    .locator("select option")
    .filter({ hasText: `${fixture.project.name} (${fixture.project.key})` });
  await expect(option).toHaveAttribute("value", fixture.project.id);
  await expect(originsPanel.locator("select")).toHaveValue(projects.suggestedId ?? "missing");
  await expect(page.getByTestId("document-comments").locator(".comment-thread__body")).toHaveText(
    fixture.comment.body,
  );
  await waitConnected(page);
  return {
    responses: responses.map((response, index) => ({
      consumer: auxiliarySuffixes[index],
      url: response.url(),
      status: response.status(),
    })),
    tags,
    origins,
    projects,
    comments,
  };
}

/** Optional root-owned PG continuation of the accepted normal native flow.
 * SQLite is explicitly blocked on normal writers at d12, never an empty-list PASS.
 * This helper creates no DB/server/native processes and changes no native body.
 */
export async function expectSelectedWikiAuxiliary(input: {
  browser: Browser;
  baseURL: string;
  ownerPage: Page;
  selected: string;
  workspaceId: string;
  document: Schema["DocumentMetaResponse"];
  creatorId: string;
  sourceBlockId: string;
  reader: Schema["SessionUserOutput"];
  persisted: Schema["BodyResponse"];
  revision: Schema["RevisionDetailResponse"];
  testInfo: TestInfo;
}): Promise<void> {
  if (input.selected !== "postgres") {
    throw new Error(
      "BLOCKED: SQLite normal writers for tags/create+assign, wiki comments, projects and document tasks still require PostgreSQL; nonempty auxiliary setup unavailable",
    );
  }
  const source = process.env.FVOCI_E2E_SELECTED_SOURCE;
  expect(source, "root must bind current integrated source").toMatch(/^[0-9a-f]{40}$/);
  expect(
    process.env.FVOCI_E2E_SELECTED_COMPILED_SOURCE,
    "fresh Rust artifact source must equal current source",
  ).toBe(source);
  const { ownerPage, workspaceId, document } = input;
  const ownerCsp = watchCspViolations(ownerPage);
  const documentPath = `/api/v1/workspaces/${workspaceId}/documents/${document.id}`;
  const fixture = await createAuxiliaryFixture(
    ownerPage,
    workspaceId,
    document,
    input.creatorId,
    input.sourceBlockId,
  );
  const ownerMounted = await observeMountedAuxiliary(ownerPage, documentPath, document, fixture);

  const context = await newCollabContext(input.browser, input.baseURL);
  let failed = true;
  try {
    const page = await context.newPage();
    const csp = watchCspViolations(page);
    await login(page, member.email, member.password);
    const identity = await page.request.get("/api/v1/auth/me");
    expect(identity.status()).toBe(200);
    const reader = (await identity.json()) as Schema["SessionUserOutput"];
    expect(reader.userId).toBe(input.reader.userId);
    expect(reader.userId).not.toBe(input.creatorId);
    expect(reader.sessionId).toMatch(UUID_RE);
    expect(reader.sessionId).not.toBe(input.reader.sessionId);
    const cookie = (await context.cookies()).find((item) => item.name === SESSION_COOKIE);
    expect(cookie?.httpOnly).toBe(true);
    expect(cookie?.sameSite).toBe("Lax");
    expect(cookie?.value).toBeTruthy();
    const freshMounted = await observeMountedAuxiliary(page, documentPath, document, fixture);
    const reloadedMounted = await observeMountedAuxiliary(page, documentPath, document, fixture);
    for (const path of [`${documentPath}/body`, `${documentPath}/revisions/${input.revision.id}`]) {
      const response = await page.request.get(path);
      expect(response.status()).toBe(200);
      expect(await response.json()).toEqual(
        path.endsWith("/body") ? input.persisted : input.revision,
      );
    }

    // A real second tenant and document, created by the normal instance-owner API.
    // The member belongs only to Acme; the owner belongs to both tenants.
    const tenantResponse = await ownerPage.request.post("/api/v1/workspaces", {
      data: {
        name: "실제 연결 권한 경계",
        slug: `aux-${randomUUID()}`,
      } satisfies Schema["CreateWorkspaceBody"],
    });
    expect(tenantResponse.status()).toBe(201);
    const tenant = (await tenantResponse.json()) as Schema["WorkspaceMetaResponse"];
    expect(tenant.id).toMatch(UUID_RE);
    expect(tenant.id).not.toBe(workspaceId);
    const foreignResponse = await ownerPage.request.post(
      `/api/v1/workspaces/${tenant.id}/documents`,
      {
        data: {
          commandId: randomUUID(),
          parentId: null,
          title: "다른 테넌트의 실제 문서",
        } satisfies Schema["CreateDocumentBody"],
      },
    );
    expect(foreignResponse.status()).toBe(201);
    const foreign = (await foreignResponse.json()) as Schema["DocumentMetaResponse"];
    expect(foreign).toMatchObject({
      workspaceId: tenant.id,
      createdBy: input.creatorId,
      projectId: null,
    });
    expect(foreign.id).toMatch(UUID_RE);
    expect(foreign.id).not.toBe(document.id);
    const foreignPath = `/api/v1/workspaces/${tenant.id}/documents/${foreign.id}`;
    const controlComment = await ownerPage.request.post(`${foreignPath}/comments`, {
      data: {
        body: "보이면 안 되는 다른 테넌트 댓글",
        mentionedUserIds: [],
        mentionedGroupIds: [],
      },
    });
    expect(controlComment.status()).toBe(201);
    const foreignComment = (await controlComment.json()) as Schema["CommentOutput"];
    expect(foreignComment.id).toMatch(UUID_RE);
    const ownerForeignRead = await ownerPage.request.get(`${foreignPath}/comments`);
    expect(ownerForeignRead.status()).toBe(200);
    expect(((await ownerForeignRead.json()) as Schema["CommentListResponse"]).items).toEqual([
      foreignComment,
    ]);
    const denials: { kind: string; suffix: string; status: number }[] = [];
    for (const suffix of auxiliarySuffixes) {
      const wrongActor = await page.request.get(`${foreignPath}/${suffix}`);
      expect(wrongActor.status()).toBe(404);
      const wrongTenant = await ownerPage.request.get(
        `/api/v1/workspaces/${tenant.id}/documents/${document.id}/${suffix}`,
      );
      expect(wrongTenant.status()).toBe(404);
      denials.push({
        kind: "wrong actor, real other tenant document",
        suffix,
        status: wrongActor.status(),
      });
      denials.push({
        kind: "owner in both tenants, mismatched document reference",
        suffix,
        status: wrongTenant.status(),
      });
    }
    // Denied reads must not poison a later authorized page mount or body/revision read.
    const afterDenialMounted = await observeMountedAuxiliary(page, documentPath, document, fixture);
    const healthyBody = await page.request.get(`${documentPath}/body`);
    expect(healthyBody.status()).toBe(200);
    expect(await healthyBody.json()).toEqual(input.persisted);
    expect(csp).toEqual([]);
    expect(ownerCsp).toEqual([]);
    await input.testInfo.attach("selected-wiki-auxiliary-mounted.json", {
      contentType: "application/json",
      body: Buffer.from(
        JSON.stringify(
          {
            scope:
              "normal API writes and mounted browser GETs; PostgreSQL only, SQLite setup BLOCKED",
            source,
            compiledSource: process.env.FVOCI_E2E_SELECTED_COMPILED_SOURCE,
            workspaceId,
            documentId: document.id,
            fixture,
            readerId: reader.userId,
            ownerMounted,
            freshMounted,
            reloadedMounted,
            afterDenialMounted,
            controlTenantId: tenant.id,
            controlDocumentId: foreign.id,
            controlCommentId: foreignComment.id,
            denials,
            nativeBodyAndManualRevisionUnchanged: true,
          },
          null,
          2,
        ),
      ),
    });
    failed = false;
  } finally {
    await closeCollabContext(context, failed);
  }
}
