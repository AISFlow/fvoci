import { randomBytes } from "node:crypto";
import { expect, test, type APIResponse, type Browser, type Page } from "@playwright/test";
import { z } from "zod";
import { createE2eUser, flowSchemas, login, readJson } from "./helpers";

const admin = { email: "Admin@Example.COM", password: "supersecret1" };
const slug = "headerdraft";
const metaSchema = flowSchemas.document.extend({ path: z.string() });
type Document = z.infer<typeof metaSchema> & { url: string; path: string };
type Kind = "wiki" | "project";

async function setup(page: Page): Promise<string> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true }))
      .or(page.getByRole("button", { name: "로그아웃", exact: true })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("검증");
    await page.getByLabel("이름", { exact: true }).fill("문서");
    await page.getByLabel("이메일").fill(admin.email);
    await page.getByLabel("비밀번호").fill(admin.password);
    await page.getByLabel("워크스페이스 이름").fill("Header draft");
    await page.getByLabel("주소(영문)").fill(slug);
    await page.getByRole("button", { name: "시작하기" }).click();
  } else if (await page.getByLabel("이메일").count()) {
    await login(page, admin.email, admin.password);
  }
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
  const workspaces = await readJson(
    await page.request.get("/api/v1/me/workspaces"),
    flowSchemas.workspaces,
  );
  const ws = workspaces.items.find((item) => item.slug === slug);
  if (!ws) throw new Error("Missing header workspace");
  return ws.id;
}

let adminCookies: Awaited<ReturnType<import("@playwright/test").BrowserContext["cookies"]>>;
test.beforeAll(async ({ browser, baseURL }) => {
  const context = await browser.newContext({ baseURL });
  try {
    const page = await context.newPage();
    await setup(page);
    adminCookies = await context.cookies();
    console.log(`TB-D runner: Bun ${process.versions.bun}; ${process.version}`);
  } finally {
    await context.close();
  }
});
test.beforeEach(async ({ page }) => {
  await page.context().addCookies(adminCookies);
});

async function fixtures(page: Page, kind: Kind) {
  const ws = await setup(page);
  let projectId: string | null = null;
  let root: string | null = null;
  let key = "";
  if (kind === "project") {
    key = `HD${randomBytes(3).toString("hex").toUpperCase()}`;
    const response = await page.request.post(`/api/v1/workspaces/${ws}/projects`, {
      data: { key, name: key, visibility: "private" },
    });
    expect(response.status()).toBe(201);
    const project = await readJson(response, flowSchemas.project);
    projectId = project.id;
    root = project.rootDocumentId;
  }
  const base = `/api/v1/workspaces/${ws}${projectId ? `/projects/${projectId}` : ""}/documents`;
  const create = async (title: string): Promise<Document> => {
    const response = await page.request.post(base, { data: { parentId: root, title } });
    expect(response.status()).toBe(201);
    const doc = await readJson(response, metaSchema);
    return {
      ...doc,
      url: `${base}/${doc.id}`,
      path: `/w/${slug}/${key || "WIKI"}-${String(doc.number)}`,
    };
  };
  const doc = await create(`${kind} original`);
  const parent = await create(`${kind} destination`);
  await page.goto(doc.path);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
  await expect(page.getByLabel("문서 제목", { exact: true })).toHaveValue(doc.title);
  await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
  return { ws, projectId, doc, parent, create };
}

async function get(page: Page, doc: Document) {
  const response = await page.request.get(doc.url);
  expect(response.status()).toBe(200);
  return readJson(response, metaSchema);
}

// Fetch first, then hold delivery: success/failure and the stored values come
// from Rust. No mocked metadata payload and no arbitrary delay controls order.
async function holdResponse(page: Page, url: string, method: string) {
  let arrived!: (response: APIResponse) => void;
  let release!: () => void;
  let retired = false;
  const entered = new Promise<APIResponse>((resolve) => {
    arrived = resolve;
  });
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const pattern = `**${url}`;
  await page.route(pattern, async (route) => {
    if (route.request().method() !== method) return route.continue();
    const actual = await route.fetch();
    arrived(actual);
    await gate;
    if (retired) return;
    await route.fulfill({ response: actual });
  });
  return {
    entered,
    release,
    retire: () => {
      retired = true;
    },
    close: async () => {
      release();
      await page.unroute(pattern);
    },
  };
}

async function move(page: Page, doc: Document, parent: Document) {
  const barrier = await holdResponse(page, `${doc.url}/move`, "POST");
  await page.getByLabel("새 위치(부모 문서)", { exact: true }).selectOption(parent.id);
  await page.getByRole("button", { name: "이동", exact: true }).click();
  expect((await barrier.entered).status()).toBe(200);
  return barrier;
}

async function freshClient(
  browser: Browser,
  page: Page,
  doc: Document,
  expected: {
    title: string;
    icon: string;
    status: string;
  },
) {
  const context = await browser.newContext({
    baseURL: new URL(page.url()).origin,
    storageState: await page.context().storageState(),
  });
  try {
    const fresh = await context.newPage();
    expect(await get(fresh, doc)).toMatchObject({
      title: expected.title,
      icon: expected.icon || null,
      status: expected.status,
    });
    await fresh.goto(doc.path);
    await expect(fresh.getByLabel("문서 제목", { exact: true })).toHaveValue(expected.title);
    await fresh.getByRole("button", { name: "문서 옵션", exact: true }).click();
    await expect(fresh.getByLabel("아이콘", { exact: true })).toHaveValue(expected.icon);
    await expect(fresh.getByLabel("문서 상태", { exact: true })).toHaveValue(expected.status);
  } finally {
    await context.close();
  }
}

for (const kind of ["wiki", "project"] as const) {
  for (const field of ["title", "icon"] as const) {
    test(`${kind}: dirty ${field} survives real Move while untouched fields synchronize`, async ({
      page,
      browser,
    }, info) => {
      const { doc, parent } = await fixtures(page, kind);
      const title = page.getByLabel("문서 제목", { exact: true });
      const icon = page.getByLabel("아이콘", { exact: true });
      const status = page.getByLabel("문서 상태", { exact: true });
      const barrier = await move(page, doc, parent);
      try {
        for (const control of [title, icon, status]) await expect(control).toBeEnabled();
        await info.attach("editable-matrix", {
          body: JSON.stringify({
            kind,
            during: "Move success held",
            title: "editable",
            icon: "editable",
            status: "editable; change saves immediately",
          }),
          contentType: "application/json",
        });
        const input = field === "title" ? title : icon;
        const draft = field === "title" ? "  preserved title  " : "  📌  ";
        await input.fill(draft);
        const serverValues =
          field === "title"
            ? { icon: "🌱", status: "published" }
            : { title: "server title", status: "published" };
        const changed = await page.request.patch(doc.url, { data: serverValues });
        expect(changed.status()).toBe(200);
        const during = await get(page, doc);
        expect(during.parentId).toBe(parent.id);
        expect(during[field]).toBe(field === "title" ? doc.title : null);
        const refreshed = page.waitForResponse(
          (r) => new URL(r.url()).pathname === doc.url && r.request().method() === "GET",
        );
        barrier.release();
        expect((await refreshed).status()).toBe(200);
        await expect(page.getByRole("button", { name: "이동", exact: true })).toBeVisible();
        await expect(status).toHaveValue("published");
        await expect(field === "title" ? icon : title).toHaveValue(
          field === "title" ? "🌱" : "server title",
        );
        await expect(input).toHaveValue(draft);
        expect((await get(page, doc))[field]).toBe(during[field]);
        const saved = page.waitForResponse(
          (r) => new URL(r.url()).pathname === doc.url && r.request().method() === "PATCH",
        );
        await input.blur();
        const response = await saved;
        expect(response.status()).toBe(200);
        const output = await readJson(response, metaSchema);
        expect(output[field]).toBe(draft.trim());
        await expect(input).toBeEnabled();
        await expect(input).toHaveValue(draft.trim());
        const after = await get(page, doc);
        expect(after[field]).toBe(draft.trim());
        await info.attach("server-readback", {
          body: JSON.stringify({ during, after }),
          contentType: "application/json",
        });
        await freshClient(browser, page, doc, {
          title: after.title,
          icon: after.icon ?? "",
          status: after.status,
        });
      } finally {
        await barrier.close();
      }
    });
  }

  test(`${kind}: status saves immediately and header controls stay disabled through its response`, async ({
    page,
    browser,
  }, info) => {
    const { doc, parent } = await fixtures(page, kind);
    const title = page.getByLabel("문서 제목", { exact: true });
    const icon = page.getByLabel("아이콘", { exact: true });
    const status = page.getByLabel("문서 상태", { exact: true });
    const moving = await move(page, doc, parent);
    const saving = await holdResponse(page, doc.url, "PATCH");
    try {
      await status.selectOption("published");
      expect((await saving.entered).status()).toBe(200);
      for (const control of [title, icon, status]) await expect(control).toBeDisabled();
      expect((await get(page, doc)).status).toBe("published");
      await info.attach("disabled-matrix", {
        body: JSON.stringify({
          kind,
          during: "status PATCH success held",
          title: "disabled",
          icon: "disabled",
          status: "disabled; already saved on server",
        }),
        contentType: "application/json",
      });
      moving.release();
      await expect(page.getByRole("button", { name: "이동", exact: true })).toBeVisible();
      for (const control of [title, icon, status]) await expect(control).toBeDisabled();
      saving.release();
      for (const control of [title, icon, status]) await expect(control).toBeEnabled();
      await expect(status).toHaveValue("published");
      await freshClient(browser, page, doc, { title: doc.title, icon: "", status: "published" });
    } finally {
      await moving.close();
      await saving.close();
    }
  });
}

// Drive a route/query lifecycle without an incidental blur save. Assertions
// still use the visible inputs, actual HTTP responses and independent GETs.
async function refresh(page: Page, queryKey: readonly string[]) {
  await page.evaluate(async (key) => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: {
        _context: {
          provides: {
            VUE_QUERY_CLIENT: {
              refetchQueries(options: { queryKey: readonly string[] }): Promise<void>;
            };
          };
        };
      };
    };
    await root.__vue_app__._context.provides.VUE_QUERY_CLIENT.refetchQueries({ queryKey: key });
  }, queryKey);
}
async function actorReentry(page: Page) {
  const before = await page.evaluate(() => performance.timeOrigin);
  const navigation = page.waitForEvent("framenavigated", {
    predicate: (frame) => frame === page.mainFrame(),
  });
  await page.evaluate(() => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: {
        _context: {
          provides: {
            VUE_QUERY_CLIENT: {
              refetchQueries(options: { queryKey: readonly string[] }): Promise<void>;
            };
          };
        };
      };
    };
    root.__vue_app__._context.provides.VUE_QUERY_CLIENT.refetchQueries({
      queryKey: ["auth", "me"],
    }).catch(reportError);
  });
  await navigation;
  await page.waitForLoadState("domcontentloaded");
  expect(await page.evaluate(() => performance.timeOrigin)).not.toBe(before);
}

async function navigate(page: Page, path: string) {
  await page.evaluate(async (next) => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: {
        config: { globalProperties: { $router: { push(path: string): Promise<void> } } };
      };
    };
    await root.__vue_app__.config.globalProperties.$router.push(next);
  }, path);
}

for (const kind of ["wiki", "project"] as const) {
  test(`${kind}: target ABA retires old draft and late Move preserves the new draft`, async ({
    page,
  }) => {
    const { doc, parent } = await fixtures(page, kind);
    const barrier = await move(page, doc, parent);
    try {
      const title = page.getByLabel("문서 제목", { exact: true });
      await title.fill("retired document draft");
      await navigate(page, parent.path);
      await expect(title).toHaveValue(parent.title);
      await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
      expect((await get(page, parent)).title).toBe(parent.title);
      await navigate(page, doc.path);
      await expect(title).toHaveValue(doc.title);
      await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
      await title.fill("new document draft");
      const delivered = page.waitForResponse(
        (r) => new URL(r.url()).pathname === `${doc.url}/move` && r.request().method() === "POST",
      );
      barrier.release();
      expect((await delivered).status()).toBe(200);
      await refresh(page, kind === "wiki" ? ["document"] : ["project-document"]);
      await expect(title).toHaveValue("new document draft");
      expect((await get(page, doc)).title).toBe(doc.title);
      const saved = page.waitForResponse(
        (r) => new URL(r.url()).pathname === doc.url && r.request().method() === "PATCH",
      );
      await title.blur();
      expect((await saved).status()).toBe(200);
      expect((await get(page, doc)).title).toBe("new document draft");
      expect((await get(page, parent)).title).toBe(parent.title);
    } finally {
      await barrier.close();
    }
  });

  test(`${kind}: actor/session ABA retires drafts while a previous actor Move is late`, async ({
    page,
  }) => {
    const { ws, projectId, doc, parent } = await fixtures(page, kind);
    const originalActor = await readJson(
      await page.request.get("/api/v1/auth/me"),
      flowSchemas.user.extend({ sessionId: z.string() }),
    );
    const member = {
      email: `header-${randomBytes(5).toString("hex")}@example.com`,
      password: "headersecret1",
    };
    createE2eUser(member.email, member.password, "헤더", {
      workspaceSlug: slug,
      membershipRole: "member",
    });
    const context = await page
      .context()
      .browser()
      ?.newContext({ baseURL: new URL(page.url()).origin });
    if (!context) throw new Error("Missing browser");
    try {
      const actor = await context.newPage();
      await login(actor, member.email, member.password);
      const user = await readJson(await actor.request.get("/api/v1/auth/me"), flowSchemas.user);
      if (projectId) {
        const response = await page.request.post(
          `/api/v1/workspaces/${ws}/projects/${projectId}/members`,
          { data: { userId: user.userId, role: "member" } },
        );
        expect(response.status()).toBe(201);
      }
      const barrier = await move(page, doc, parent);
      try {
        const title = page.getByLabel("문서 제목", { exact: true });
        await title.fill("old actor draft");
        await page.context().addCookies(await context.cookies());
        expect(
          (await readJson(await page.request.get("/api/v1/auth/me"), flowSchemas.user)).userId,
        ).toBe(user.userId);
        barrier.retire();
        await actorReentry(page);
        barrier.release();
        expect((await get(page, doc)).title).toBe(doc.title);
        await page.goto(doc.path);
        await expect(title).toHaveValue(doc.title);
        await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
        await title.fill("member actor draft");
        expect((await page.request.post("/api/v1/auth/login", { data: admin })).status()).toBe(200);
        const returnedActor = await readJson(
          await page.request.get("/api/v1/auth/me"),
          flowSchemas.user.extend({ sessionId: z.string() }),
        );
        expect(returnedActor.userId).toBe(originalActor.userId);
        expect(returnedActor.sessionId).not.toBe(originalActor.sessionId);
        await actorReentry(page);
        expect((await get(page, doc)).title).toBe(doc.title);
        await page.goto(doc.path);
        await expect(title).toHaveValue(doc.title);
        await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
        await title.fill("current actor draft");
        await refresh(page, kind === "wiki" ? ["document"] : ["project-document"]);
        await expect(title).toHaveValue("current actor draft");
        expect((await get(page, doc)).title).toBe(doc.title);
        const saved = page.waitForResponse(
          (r) => new URL(r.url()).pathname === doc.url && r.request().method() === "PATCH",
        );
        await title.blur();
        expect((await saved).status()).toBe(200);
        expect((await get(page, doc)).title).toBe("current actor draft");
      } finally {
        await barrier.close();
      }
    } finally {
      await context.close();
    }
  });

  test(`${kind}: real deleted save failure arrives after ABA without replacing the current draft`, async ({
    page,
  }) => {
    const { doc, parent } = await fixtures(page, kind);
    const title = page.getByLabel("문서 제목", { exact: true });
    const archived = await page.request.post(`${doc.url}/trash`);
    expect(archived.status()).toBe(200);
    const failure = await holdResponse(page, doc.url, "PATCH");
    try {
      await expect(title).toBeEnabled();
      await title.fill("rejected old draft");
      await title.blur();
      expect((await failure.entered).status()).toBe(404);
      expect((await page.request.get(doc.url)).status()).toBe(404);
      await navigate(page, parent.path);
      await expect(title).toHaveValue(parent.title);
      await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
      const restored = await page.request.post(`${doc.url}/restore`);
      expect(restored.status()).toBe(200);
      await navigate(page, doc.path);
      await expect(title).toHaveValue(doc.title);
      await expect(title).toBeEnabled();
      await title.fill("new draft after failed save");
      const delivered = page.waitForResponse(
        (r) => new URL(r.url()).pathname === doc.url && r.request().method() === "PATCH",
      );
      failure.release();
      expect((await delivered).status()).toBe(404);
      await expect(title).toHaveValue("new draft after failed save");
      await expect(page.locator(".document-page__error")).toHaveCount(0);
      await failure.close();
      const saved = page.waitForResponse(
        (r) => new URL(r.url()).pathname === doc.url && r.request().method() === "PATCH",
      );
      await title.blur();
      expect((await saved).status()).toBe(200);
      expect((await get(page, doc)).title).toBe("new draft after failed save");
      expect((await get(page, parent)).title).toBe(parent.title);
    } finally {
      await failure.close();
    }
  });

  test(`${kind}: read-only transition discards dirty fields before a late Move`, async ({
    page,
  }) => {
    const { doc, parent } = await fixtures(page, kind);
    const title = page.getByLabel("문서 제목", { exact: true });
    const icon = page.getByLabel("아이콘", { exact: true });
    const status = page.getByLabel("문서 상태", { exact: true });
    const barrier = await move(page, doc, parent);
    try {
      await title.fill("retired permission draft");
      expect((await page.request.patch(doc.url, { data: { status: "archived" } })).status()).toBe(
        200,
      );
      await refresh(page, kind === "wiki" ? ["document"] : ["project-document"]);
      for (const control of [title, icon, status]) await expect(control).toBeDisabled();
      await expect(title).toHaveValue(doc.title);
      const delivered = page.waitForResponse(
        (r) => new URL(r.url()).pathname === `${doc.url}/move` && r.request().method() === "POST",
      );
      barrier.release();
      expect((await delivered).status()).toBe(200);
      await expect(title).toHaveValue(doc.title);
      expect((await get(page, doc)).title).toBe(doc.title);
      expect((await page.request.patch(doc.url, { data: { status: "draft" } })).status()).toBe(200);
      await refresh(page, kind === "wiki" ? ["document"] : ["project-document"]);
      await expect(title).toBeEnabled();
      await expect(title).toHaveValue(doc.title);
      expect((await get(page, doc)).title).toBe(doc.title);
    } finally {
      await barrier.close();
    }
  });
}

for (const kind of ["wiki", "project"] as const) {
  test(`${kind}: membership revocation denies writes and retires a draft during late Move`, async ({
    page,
    browser,
  }) => {
    const { ws, projectId, doc, parent } = await fixtures(page, kind);
    const member = {
      email: `revoked-${randomBytes(5).toString("hex")}@example.com`,
      password: "headersecret1",
    };
    createE2eUser(member.email, member.password, "철회", {
      workspaceSlug: slug,
      membershipRole: "member",
    });
    const context = await browser.newContext({ baseURL: new URL(page.url()).origin });
    try {
      const editing = await context.newPage();
      await login(editing, member.email, member.password);
      const user = await readJson(await editing.request.get("/api/v1/auth/me"), flowSchemas.user);
      if (projectId) {
        expect(
          (
            await page.request.post(`/api/v1/workspaces/${ws}/projects/${projectId}/members`, {
              data: { userId: user.userId, role: "member" },
            })
          ).status(),
        ).toBe(201);
      }
      await editing.goto(doc.path);
      await expect(editing.locator('[data-collab-status="connected"]')).toBeVisible();
      await editing.getByRole("button", { name: "문서 옵션", exact: true }).click();
      const barrier = await move(editing, doc, parent);
      try {
        await editing.getByLabel("문서 제목", { exact: true }).fill("revoked actor draft");
        expect(
          (await page.request.delete(`/api/v1/workspaces/${ws}/members/${user.userId}`)).status(),
        ).toBe(200);
        expect(
          (await editing.request.patch(doc.url, { data: { title: "forbidden write" } })).status(),
        ).toBe(404);
        await editing.waitForURL(/\?denied=workspace$/);
        await expect(editing.getByLabel("문서 제목", { exact: true })).toHaveCount(0);
        barrier.release();
        expect((await get(page, doc)).title).toBe(doc.title);
        expect((await editing.request.get(doc.url)).status()).toBe(404);
        const fresh = await browser.newContext({
          baseURL: new URL(page.url()).origin,
          storageState: await context.storageState(),
        });
        try {
          const denied = await fresh.newPage();
          expect((await denied.request.get(doc.url)).status()).toBe(404);
        } finally {
          await fresh.close();
        }
        expect((await get(page, doc)).title).toBe(doc.title);
      } finally {
        await barrier.close();
      }
    } finally {
      await context.close();
    }
  });
}
