import { createHmac } from "node:crypto";
import { createServer, type IncomingHttpHeaders, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { expect, type Page, test } from "@playwright/test";
import { login } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

type Delivery = { headers: IncomingHttpHeaders; body: string };

// The e2e server runs with FVOCI_WEBHOOK_ALLOW_TARGETS=127.0.0.1 so this local receiver is allowed.
async function startReceiver(): Promise<{ server: Server; port: number; deliveries: Delivery[] }> {
  const deliveries: Delivery[] = [];
  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      if (req.method === "POST") {
        deliveries.push({ headers: req.headers, body: Buffer.concat(chunks).toString("utf8") });
      }
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("ok");
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return { server, port, deliveries };
}

function signatureMatches(delivery: Delivery, secret: string): boolean {
  const expected = `sha256=${createHmac("sha256", secret).update(delivery.body, "utf8").digest("hex")}`;
  return delivery.headers["x-fvoci-signature"] === expected;
}

function verbOf(delivery: Delivery): string | null {
  try {
    const parsed = JSON.parse(delivery.body) as { verb?: unknown };
    return typeof parsed.verb === "string" ? parsed.verb : null;
  } catch {
    return null;
  }
}

test("owner creates a signed webhook, receives project.created, then deletes it", async ({ page }) => {
  test.setTimeout(90_000);
  const receiver = await startReceiver();
  try {
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

    const hookUrl = `http://127.0.0.1:${receiver.port}/hook`;
    await page.goto(`/w/${owner.workspaceSlug}/settings`);

    // No GitHub App is configured under e2e: the section only reports the disconnected state.
    const github = page.locator("details").filter({
      has: page.locator("summary").filter({ hasText: /^GitHub$/ }),
    });
    await github.locator("summary").click();
    await expect(github.getByText("연결되지 않음")).toBeVisible();
    await expect(github.getByRole("button", { name: "GitHub App 설치" })).toBeVisible();

    const webhooks = page.locator("details").filter({
      has: page.locator("summary").filter({ hasText: /^웹훅$/ }),
    });
    await webhooks.locator("summary").click();
    await expect(webhooks.getByText("웹훅이 없습니다")).toBeVisible();
    await webhooks.getByRole("textbox", { name: "URL", exact: true }).fill(hookUrl);
    await webhooks.getByLabel("프로젝트 생성", { exact: true }).check();
    await webhooks.getByRole("button", { name: "추가", exact: true }).click();

    await expect(page.getByRole("status").filter({ hasText: "시크릿은 지금만 보입니다" })).toBeVisible();
    const revealed = page.getByRole("textbox", { name: "웹훅 서명 시크릿" });
    await expect(revealed).toBeVisible();
    const secret = await revealed.inputValue();
    expect(secret.length).toBeGreaterThan(16);
    await expect(page.getByText(hookUrl, { exact: true })).toBeVisible();

    await page.reload();
    await webhooks.locator("summary").click();
    await expect(page.getByText(hookUrl, { exact: true })).toBeVisible();
    await expect(page.getByRole("textbox", { name: "웹훅 서명 시크릿" })).toHaveCount(0);
    await expect(page.getByText("시크릿은 지금만 보입니다")).toHaveCount(0);

    const workspacesRes = await page.request.get("/api/v1/me/workspaces");
    expect(workspacesRes.ok()).toBe(true);
    const workspace = (await workspacesRes.json()).items.find(
      (item: { slug: string }) => item.slug === owner.workspaceSlug,
    );
    expect(workspace).toBeTruthy();
    const projectRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
      data: { key: "HOOK", name: "웹훅 프로젝트", visibility: "workspace" },
    });
    expect(projectRes.status(), await projectRes.text()).toBe(201);

    await expect
      .poll(
        () =>
          receiver.deliveries.some(
            (delivery) => verbOf(delivery) === "project.created" && signatureMatches(delivery, secret),
          ),
        { timeout: 20_000 },
      )
      .toBe(true);

    await page.getByRole("button", { name: `삭제 ${hookUrl}` }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog.getByRole("heading", { name: "웹훅을 삭제할까요?" })).toBeVisible();
    await dialog.getByRole("button", { name: "삭제", exact: true }).click();
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await expect(page.getByText("웹훅이 없습니다")).toBeVisible();
    await expect(page.getByText(hookUrl, { exact: true })).toHaveCount(0);

    const listRes = await page.request.get(`/api/v1/workspaces/${workspace.id}/webhooks`);
    expect(listRes.ok()).toBe(true);
    expect((await listRes.json()).items).toEqual([]);
  } finally {
    receiver.server.closeAllConnections();
    await new Promise<void>((resolve) => receiver.server.close(() => resolve()));
  }
});

const INSTANCE_PATH = "/api/v1/instance";

function isInstanceRead(url: string): boolean {
  return new URL(url).pathname === INSTANCE_PATH;
}

/** The webhook test above runs setup first; alone (or after its failure) this still signs in. */
async function signInOwner(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if ((await page.getByRole("button", { name: "시작하기" }).count()) > 0) {
    await page.getByLabel("성").fill(owner.familyName);
    await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
    await page.getByLabel("이메일").fill(owner.email);
    await page.getByLabel("비밀번호").fill(owner.password);
    await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
    await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else if ((await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0) {
    await login(page, owner.email, owner.password);
  }
}

test("public features.ai gates the document AI menu; the server keeps its own AI gate", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  await signInOwner(page);

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspace = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === owner.workspaceSlug,
  ) as { id: string };
  const docRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/documents`, {
    data: { parentId: null, title: "AI 게이트 문서" },
  });
  expect(docRes.status(), await docRes.text()).toBe(201);
  const doc = (await docRes.json()) as { id: string; number: number };
  const docPath = `/w/${owner.workspaceSlug}/WIKI-${doc.number}`;
  const aiMenu = page.getByRole("group", { name: "AI 도구" });

  // Default instance: the public view says off and the menu is absent once that answer is in.
  const initial = await request.get(INSTANCE_PATH);
  expect(initial.ok()).toBe(true);
  expect((await initial.json()).values.features.ai).toBe(false);
  const instanceRead = page.waitForResponse((res) => isInstanceRead(res.url()));
  await page.goto(docPath);
  await instanceRead;
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(aiMenu).toHaveCount(0);

  // Turn it on from the real admin settings card without reloading the app, so the
  // document screen must pick the change up from the shared ["instance"] query.
  // WHY: the document shell has no admin link; a same-document history push keeps the SPA cache.
  await page.evaluate((path) => {
    window.history.pushState(null, "", path);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, "/settings/admin");
  const features = page.getByRole("region", { name: "기능 토글", exact: true });
  await expect(features.getByLabel("features.ai")).toBeEnabled();
  await expect(features.getByLabel("features.ai")).not.toBeChecked();
  await features.getByLabel("features.ai").check();
  const saved = page.waitForResponse(
    (res) => res.url().endsWith("/api/v1/admin/instance-settings") && res.request().method() === "PATCH",
  );
  await features.getByRole("button", { name: "저장", exact: true }).click();
  expect((await saved).status()).toBe(200);

  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`${docPath}$`));
  await expect(aiMenu).toBeVisible();
  await page.reload();
  await expect(aiMenu).toBeVisible();

  // The flag is public and changes only the UI; anonymous callers still cannot write settings.
  const publicView = await request.get(INSTANCE_PATH);
  expect((await publicView.json()).values.features.ai).toBe(true);
  const anonPatch = await request.patch("/api/v1/admin/instance-settings", {
    data: { features: { ai: false } },
  });
  expect([401, 403]).toContain(anonPatch.status());

  // The server's own AI gate (FVOCI_AI_ENABLED/FVOCI_AI_SECRET) is unset in e2e: the menu shows
  // but the action answers 503 ai_unavailable and the buttons lock.
  const direct = await page.request.post(`/api/v1/workspaces/${workspace.id}/ai/summarize`, {
    data: { documentId: doc.id },
  });
  expect(direct.status()).toBe(503);
  expect((await direct.json()).code).toBe("ai_unavailable");
  await aiMenu.getByRole("button", { name: "요약", exact: true }).click();
  await expect(page.getByRole("alert").filter({ hasText: "AI 기능을 지금 사용할 수 없습니다" })).toBeVisible();
  await expect(aiMenu.getByRole("button", { name: "요약", exact: true })).toBeDisabled();

  // Back to the default: the card's reset writes null and the menu disappears again.
  await page.goto("/settings/admin");
  const reset = page.getByRole("region", { name: "기능 토글", exact: true });
  const cleared = page.waitForResponse(
    (res) => res.url().endsWith("/api/v1/admin/instance-settings") && res.request().method() === "PATCH",
  );
  await reset.getByRole("button", { name: "기본값으로", exact: true }).click();
  expect((await cleared).status()).toBe(200);
  const offRead = page.waitForResponse((res) => isInstanceRead(res.url()));
  await page.goto(docPath);
  await offRead;
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(aiMenu).toHaveCount(0);
});

type BodyNode = { type?: string; text?: string; attrs?: { id?: string }; content?: BodyNode[] };

/**
 * Top-level blocks of a stored body in order: a paragraph as its text (mentions as `@<id>`,
 * empty paragraphs as ""), any other block as `type[paragraph|texts]`.
 */
function blocksOf(root: BodyNode): string[] {
  const inline = (child: BodyNode): string =>
    child.type === "text"
      ? (child.text ?? "")
      : child.type === "mention"
        ? `@${child.attrs?.id ?? ""}`
        : (child.content ?? []).map(inline).join("");
  const paragraphs = (node: BodyNode): string[] =>
    node.type === "paragraph" ? [inline(node)] : (node.content ?? []).flatMap(paragraphs);
  return (root.content ?? []).map((node) =>
    node.type === "paragraph" ? inline(node) : `${node.type}[${paragraphs(node).join("|")}]`,
  );
}

test("confirmed AI results apply through the live editor and the project task route", async ({ page }) => {
  test.setTimeout(120_000);
  await signInOwner(page);
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  const workspace = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === owner.workspaceSlug,
  ) as { id: string };
  const wsId = workspace.id;

  const on = await page.request.patch("/api/v1/admin/instance-settings", {
    data: { features: { ai: true } },
  });
  expect(on.status(), await on.text()).toBe(200);
  try {
    const projectRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
      data: { key: "AIAP", name: "AI 적용 프로젝트", visibility: "workspace" },
    });
    expect(projectRes.status(), await projectRes.text()).toBe(201);
    const project = (await projectRes.json()) as { id: string; rootDocumentId: string };
    const docRes = await page.request.post(
      `/api/v1/workspaces/${wsId}/projects/${project.id}/documents`,
      { data: { parentId: project.rootDocumentId, title: "AI 적용 문서" } },
    );
    expect(docRes.status(), await docRes.text()).toBe(201);
    const doc = (await docRes.json()) as { id: string; displayId: string };
    const linkRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
      data: { parentId: null, title: "연결 후보 문서" },
    });
    expect(linkRes.status(), await linkRes.text()).toBe(201);
    const linkDoc = (await linkRes.json()) as { id: string; number: number };

    // WHY: the server's own AI gate is off under e2e (the gate test above proves the real route
    // answers 503; the real summarize/generate/suggest outputs are covered by the Rust
    // integrations tests). Only these three AI answers are stubbed — the apply path below uses the
    // real editor, collab persistence, task route and authorization.
    const aiCalls: string[] = [];
    await page.route(/\/api\/v1\/workspaces\/[^/]+\/ai\/(summarize|generate-tasks|suggest-links)$/, (route) => {
      const kind = new URL(route.request().url()).pathname.split("/").pop()!;
      expect(route.request().postDataJSON()).toEqual({ documentId: doc.id });
      aiCalls.push(kind);
      const body =
        kind === "summarize"
          ? { summary: "요약 첫 줄 🙂\n\n요약 둘째 줄" }
          : kind === "generate-tasks"
            ? { titles: ["AI 작업 가", "AI 작업 나", "AI 작업 다"] }
            : { documentIds: [linkDoc.id] };
      return route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    });

    const bodyOf = async () => {
      const res = await page.request.get(
        `/api/v1/workspaces/${wsId}/projects/${project.id}/documents/${doc.id}/body`,
      );
      expect(res.ok()).toBe(true);
      return blocksOf((await res.json()).contentJson as BodyNode);
    };

    await page.goto(`/w/${owner.workspaceSlug}/${doc.displayId}`);
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
    const editor = page.locator(".fvoci-editor .ProseMirror");
    await editor.click();
    // The body ends in a bullet list with the caret inside it: results must go after the list,
    // not into the item or the caret position.
    await page.keyboard.type("기존 본문");
    await page.keyboard.press("Enter");
    await page.keyboard.type("- 항목 하나");
    await expect(editor.locator("ul li")).toHaveCount(1);
    await page.keyboard.press("Home");

    const aiMenu = page.getByRole("group", { name: "AI 도구" });
    const preview = page.getByRole("region", { name: "AI 결과 미리보기" });

    // Summary: nothing changes until the user confirms; a double click inserts once, at the end.
    await aiMenu.getByRole("button", { name: "요약", exact: true }).click();
    await expect(preview.getByText("요약 첫 줄 🙂")).toBeVisible();
    await expect(editor).not.toContainText("요약 첫 줄");
    await preview.getByRole("button", { name: "본문 끝에 삽입" }).dblclick();
    await expect(page.getByRole("status").filter({ hasText: "본문에 삽입했습니다." })).toBeVisible();
    await expect(preview).toHaveCount(0);

    // Links: document mentions appended after the summary.
    await aiMenu.getByRole("button", { name: "링크 제안", exact: true }).click();
    await expect(preview.getByText("연결 후보 문서")).toBeVisible();
    await preview.getByRole("button", { name: "링크 삽입" }).click();
    await expect(editor.locator("[data-mention]")).toHaveCount(1);

    await page.getByRole("button", { name: "저장", exact: true }).click();
    await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });
    await expect
      .poll(bodyOf, { timeout: 15_000 })
      .toEqual([
        "기존 본문",
        "bulletList[항목 하나]",
        "요약 첫 줄 🙂",
        "요약 둘째 줄",
        `@${linkDoc.id}`,
      ]);

    // Tasks: the second create is rejected once (injected 422 before the server); the retry
    // creates only what is still missing, so the first title is not recreated.
    let rejectOnce = true;
    const taskPosts: string[] = [];
    await page.route(`**/api/v1/workspaces/${wsId}/projects/${project.id}/tasks`, async (route) => {
      if (route.request().method() !== "POST") return route.fallback();
      const { title } = route.request().postDataJSON() as { title: string };
      taskPosts.push(title);
      if (title === "AI 작업 나" && rejectOnce) {
        rejectOnce = false;
        return route.fulfill({
          status: 422,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "invalid", status: 422, code: "validation_failed" }),
        });
      }
      return route.fallback();
    });
    await aiMenu.getByRole("button", { name: "태스크 생성", exact: true }).click();
    await expect(preview.getByText("AI 작업 가")).toBeVisible();
    await preview.getByRole("button", { name: "태스크 만들기" }).click();
    await expect(page.getByRole("status").filter({ hasText: "태스크 1개를 만들었습니다." })).toBeVisible();
    await expect(page.locator(".document-ai-menu [role=alert]")).toBeVisible();
    await expect(preview.locator('[data-ai-task-state="created"]')).toHaveCount(1);
    await preview.getByRole("button", { name: "태스크 만들기" }).click();
    await expect(page.getByRole("status").filter({ hasText: "태스크 3개를 만들었습니다." })).toBeVisible();
    await expect(preview).toHaveCount(0);
    expect(taskPosts).toEqual(["AI 작업 가", "AI 작업 나", "AI 작업 나", "AI 작업 다"]);
    const tasksRes = await page.request.get(`/api/v1/workspaces/${wsId}/projects/${project.id}/tasks`);
    expect(tasksRes.ok()).toBe(true);
    const titles = ((await tasksRes.json()).items as Array<{ title: string }>)
      .map((item) => item.title)
      .filter((title) => title.startsWith("AI 작업"))
      .sort();
    expect(titles).toEqual(["AI 작업 가", "AI 작업 나", "AI 작업 다"]);

    // A result belongs to its document: switching documents drops the pending preview.
    await aiMenu.getByRole("button", { name: "요약", exact: true }).click();
    await expect(preview).toBeVisible();
    await page.evaluate((path) => {
      window.history.pushState(null, "", path);
      window.dispatchEvent(new PopStateEvent("popstate"));
    }, `/w/${owner.workspaceSlug}/WIKI-${linkDoc.number}`);
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
    await expect(preview).toHaveCount(0);
    // Wiki documents have no project: generating tasks stays locked there.
    await expect(aiMenu.getByRole("button", { name: "태스크 생성", exact: true })).toBeDisabled();
    await expect(page.getByText("프로젝트에 속한 문서에서만 태스크를 만들 수 있습니다.")).toBeVisible();
    expect(aiCalls).toEqual(["summarize", "suggest-links", "generate-tasks", "summarize"]);
  } finally {
    const off = await page.request.patch("/api/v1/admin/instance-settings", {
      data: { features: { ai: null } },
    });
    expect(off.status(), await off.text()).toBe(200);
  }
});
