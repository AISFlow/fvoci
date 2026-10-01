import {
  expect,
  test,
  type APIRequestContext,
  type Locator,
  type Page,
  type Request,
  type Response,
} from "@playwright/test";
import { readJson, flowSchemas, createE2eUser } from "./helpers";
import {
  admin,
  createDoc,
  editorOf,
  newSignedInPage,
  openDoc,
  save,
  setupInstance,
  workspaceId,
  type TiptapNode,
} from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });
const guest = {
  email: "entities-guest@example.com",
  password: "guestpass1",
  givenName: "참조손님",
};

test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
  createE2eUser(guest.email, guest.password, guest.givenName, {
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "guest",
  });
});
type Item = {
  id: string;
  number: number;
  title: string;
};
async function fixtures(request: APIRequestContext, key: string) {
  const ws = await workspaceId(request);
  const projectResponse = await request.post(`/api/v1/workspaces/${ws}/projects`, {
    data: { key, name: `${key} project`, visibility: "workspace" },
  });
  expect(projectResponse.status(), await projectResponse.text()).toBe(201);
  const project = (await readJson(projectResponse, flowSchemas.project)) as {
    id: string;
    rootDocumentId: string;
  };
  const wiki = await createDoc(request, ws, `${key} wiki reference`);
  const documentResponse = await request.post(
    `/api/v1/workspaces/${ws}/projects/${project.id}/documents`,
    {
      data: { title: `${key} project reference`, parentId: project.rootDocumentId },
    },
  );
  expect(documentResponse.status(), await documentResponse.text()).toBe(201);
  const document = (await readJson(documentResponse, flowSchemas.document)) as Item;
  const taskResponse = await request.post(`/api/v1/workspaces/${ws}/projects/${project.id}/tasks`, {
    data: { title: `${key} task reference` },
  });
  expect(taskResponse.status(), await taskResponse.text()).toBe(201);
  const task = (await readJson(taskResponse, flowSchemas.item)) as Item;
  return { ws, project, wiki, document, task };
}

async function nextParagraph(page: Page, editor: Locator) {
  // Clicking the editor's center can open the previous embed's textarea.
  // Target a paragraph so native typing/paste reaches the ProseMirror host.
  await editor.locator(":scope > p").last().click();
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await expect(editor).toBeFocused();
}
async function mention(page: Page, query: string, title: string) {
  await page.keyboard.type(`@${query}`);
  await page.locator(".fvoci-suggestion").getByRole("option", { name: title, exact: true }).click();
}
async function slash(page: Page, displayId: string) {
  await page.keyboard.type(`/${displayId}`);
  await page
    .locator(".fvoci-suggestion")
    .getByRole("option")
    .filter({ hasText: displayId })
    .click();
}
async function paste(page: Page, ref: string) {
  const url = new URL(`/w/${admin.workspaceSlug}/${ref}`, page.url()).href;
  await page.evaluate(async (text) => navigator.clipboard.writeText(text), url);
  await page.keyboard.press("Control+V");
}
function nodes(body: TiptapNode, type: string): TiptapNode[] {
  return [
    ...(body.type === type ? [body] : []),
    ...(body.content ?? []).flatMap((child) => nodes(child, type)),
  ];
}
async function body(request: APIRequestContext, path: string): Promise<TiptapNode> {
  const response = await request.get(path);
  expect(response.ok(), await response.text()).toBe(true);
  const data = await readJson(response, flowSchemas.body);
  return data.contentJson as TiptapNode;
}

for (const host of ["wiki", "project", "task"] as const) {
  test(`${host}: real @, slash, UUID/display paste persist and resolve after reload`, async ({
    browser,
    baseURL,
  }) => {
    const signed = await newSignedInPage(browser, baseURL, admin, {
      permissions: ["clipboard-read", "clipboard-write"],
    });
    const page = signed.page;
    try {
      const key = { wiki: "ENW", project: "ENP", task: "ENT" }[host];
      const f = await fixtures(page.request, key);
      if (host === "wiki") {
        const response = await page.request.post(`/api/v1/workspaces/${f.ws}/groups`, {
          data: { name: "Entity Team" },
        });
        expect(response.status(), await response.text()).toBe(201);
      }
      const path =
        host === "wiki"
          ? f.wiki.path
          : `/w/${admin.workspaceSlug}/${key}-${String(host === "project" ? f.document.number : f.task.number)}`;
      const hostPath = path;
      const editor = await openDoc(page, hostPath);
      await expect(editor).toHaveAttribute("contenteditable", "true");
      await editor.click();
      await mention(page, `WIKI-${String(f.wiki.number)}`, `${key} wiki reference`);
      await expect(editor.locator("[data-mention]")).toHaveText(`@${key} wiki reference`);
      await nextParagraph(page, editor);
      await slash(page, `${key}-${String(f.task.number)}`);
      await expect(editor.locator('.afn-embed[data-entity="task"]')).toContainText(f.task.title);
      await nextParagraph(page, editor);
      await paste(page, f.document.id);
      await expect(editor.locator('.afn-embed[data-entity="document"]').first()).toContainText(
        f.document.title,
      );
      await nextParagraph(page, editor);
      await paste(page, `WIKI-${String(f.wiki.number)}`);
      await expect(editor.locator('.afn-embed[data-entity="document"]').last()).toContainText(
        `${key} wiki reference`,
      );
      await save(page);
      const bodyPath =
        host === "wiki"
          ? `/api/v1/workspaces/${f.ws}/documents/${f.wiki.id}/body`
          : host === "project"
            ? `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents/${f.document.id}/body`
            : `/api/v1/workspaces/${f.ws}/tasks/${f.task.id}`;
      await expect
        .poll(async () => nodes(await body(page.request, bodyPath), "embed").length)
        .toBe(3);
      const saved = await body(page.request, bodyPath);
      const required1 = nodes(saved, "mention")[0];
      if (required1 === undefined) {
        throw new Error('Missing fixture value: nodes(saved, "mention")[0]');
      }
      expect(required1.attrs).toMatchObject({
        entity: "document",
        id: f.wiki.id,
        label: `WIKI-${String(f.wiki.number)}`,
      });
      expect(
        nodes(saved, "embed").map((n) => {
          const required2 = n.attrs;
          if (required2 === undefined) {
            throw new Error("Missing fixture value: n.attrs");
          }
          const required3 = n.attrs;
          if (required3 === undefined) {
            throw new Error("Missing fixture value: n.attrs");
          }
          return { entity: required2.entity, ref: required3.ref };
        }),
      ).toEqual([
        { entity: "task", ref: f.task.id },
        { entity: "document", ref: f.document.id },
        { entity: "document", ref: `WIKI-${String(f.wiki.number)}` },
      ]);
      if (host === "wiki") {
        const peer = await newSignedInPage(browser, baseURL, admin);
        try {
          const peerEditor = await openDoc(peer.page, hostPath);
          await expect(peerEditor.locator("[data-mention]")).toHaveText(`@${key} wiki reference`);
          await expect(peerEditor.locator(".afn-embed")).toHaveCount(3);
          await nextParagraph(page, editor);
          await mention(page, `${key}-${String(f.task.number)}`, f.task.title);
          await expect(peerEditor.locator("[data-mention]").last()).toHaveText(`@${f.task.title}`);
          await nextParagraph(page, editor);
          await mention(page, "편집", "동료편집");
          await nextParagraph(page, editor);
          await mention(page, "Entity", "Entity Team");
          await expect(peerEditor.locator("[data-mention]").last()).toHaveText("@Entity Team");
          await save(page);
          const shared = await body(page.request, bodyPath);
          expect(
            nodes(shared, "mention").map((node) => {
              const required4 = node.attrs;
              if (required4 === undefined) {
                throw new Error("Missing fixture value: node.attrs");
              }
              return required4.entity;
            }),
          ).toEqual(["document", "task", "user", "group"]);
        } finally {
          await peer.context.close();
        }
      }
      await page.reload();
      await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({
        timeout: 15000,
      });
      await expect(editorOf(page).locator('.afn-embed[data-entity="task"]')).toContainText(
        f.task.title,
      );
      await expect(
        editorOf(page).locator('.afn-embed[data-entity="document"]').first(),
      ).toContainText(f.document.title);
      const reloaded = await body(page.request, bodyPath);
      expect(nodes(reloaded, "embed").map((n) => n.attrs)).toEqual(
        nodes(saved, "embed").map((n) => n.attrs),
      );
    } finally {
      await signed.context.close();
    }
  });
}

test("guest member denial keeps allowed entities; inaccessible refs and readonly retain permission boundaries", async ({
  browser,
  baseURL,
}) => {
  const owner = await newSignedInPage(browser, baseURL, admin);
  const visitor = await newSignedInPage(browser, baseURL, guest, {
    permissions: ["clipboard-read", "clipboard-write"],
  });
  try {
    const f = await fixtures(owner.page.request, "ENG");
    const members = await owner.page.request.get(`/api/v1/workspaces/${f.ws}/members`);
    const fixtureValue1 = (await readJson(members, flowSchemas.members)).items.find(
      (m: { email: string }) => m.email === guest.email,
    );
    if (fixtureValue1 === undefined)
      throw new Error(
        "Missing fixture value: (await readJson(members, flowSchemas.members)).items.find(\n      (m: { email: string }) => m.email === guest.email,\n    )",
      );
    const guestId = fixtureValue1.userId;
    const grant = `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/members`;
    expect(
      (await owner.page.request.post(grant, { data: { userId: guestId, role: "member" } })).ok(),
    ).toBe(true);
    const page = visitor.page;
    const editor = await openDoc(
      page,
      `/w/${admin.workspaceSlug}/ENG-${String(f.document.number)}`,
    );
    expect((await page.request.get(`/api/v1/workspaces/${f.ws}/members`)).status()).toBe(404);
    await editor.click();
    await mention(page, `ENG-${String(f.task.number)}`, f.task.title);
    await expect(editor.locator("[data-mention]")).toHaveText(`@${f.task.title}`);
    await nextParagraph(page, editor);
    await paste(page, f.wiki.id);
    await expect(editor.locator(".afn-embed-inaccessible")).toBeVisible();
    await expect(editor).not.toContainText("ENG wiki reference");
    await nextParagraph(page, editor);
    await page.keyboard.type("/");
    await expect(page.locator(".fvoci-suggestion").getByRole("option").first()).toBeVisible();
    await page.keyboard.press("Escape");
    await page.keyboard.press("Backspace");
    await save(page);
    // Revoke editing through the real project membership API, then revalidate.
    expect(
      (await owner.page.request.patch(`${grant}/${guestId}`, { data: { role: "viewer" } })).ok(),
    ).toBe(true);
    await page.reload();
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15000 });
    await expect(editorOf(page)).toHaveAttribute("contenteditable", "false");
    await expect(editorOf(page).locator(".afn-embed-inaccessible")).toBeVisible();
    const before = await body(
      page.request,
      `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents/${f.document.id}/body`,
    );
    await editorOf(page).click();
    await page.keyboard.type("@ENG-1");
    expect(
      await body(
        page.request,
        `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents/${f.document.id}/body`,
      ),
    ).toEqual(before);
    await expect(page.locator(".fvoci-suggestion")).toHaveCount(0);
  } finally {
    await visitor.context.close();
    await owner.context.close();
  }
});

// Delay delivery, not execution: every held response comes from the real Rust
// server/DB. Edits and peer presence then replace the computed session while
// the document lifecycle mutation is still waiting for its HTTP response.
async function holdResponse(page: Page, path: string, deferRequest = false) {
  let forward!: () => void;
  const requestGate = new Promise<void>((resolve) => {
    forward = resolve;
  });
  if (!deferRequest) {
    forward();
  }
  let release!: () => void;
  const wait = new Promise<void>((resolve) => {
    release = resolve;
  });
  let received!: (status: number) => void;
  const response = new Promise<number>((resolve) => {
    received = resolve;
  });
  await page.route(
    `**${path}`,
    async (route) => {
      await requestGate;
      const result = await route.fetch();
      received(result.status());
      await wait;
      await route.fulfill({ response: result });
    },
    { times: 1 },
  );
  return {
    response,
    release: () => {
      forward();
      release();
    },
    forward,
  };
}

async function retainedCounts(page: Page, ws: string, refresh = false) {
  // Inspect the existing app's actual QueryClient; no product test hook or
  // replacement client. query must honor the retained 30-second cache.
  return page.evaluate(
    async ({ workspaceId, refresh }) => {
      type Client = import("@tanstack/vue-query").QueryClient;
      const root = document.getElementById("root") as HTMLElement & {
        __vue_app__: {
          _context: {
            provides: Record<string, Client>;
          };
        };
      };
      const required5 = root.__vue_app__._context.provides.VUE_QUERY_CLIENT;
      if (required5 === undefined) {
        throw new Error(
          "Missing fixture value: root.__vue_app__._context.provides.VUE_QUERY_CLIENT",
        );
      }
      const client = required5;
      const counts = await client.query({
        queryKey: ["me", "workspaces"],
        staleTime: refresh ? 0 : 30000,
        queryFn: async () => {
          const response = await fetch("/api/v1/me/workspaces");
          if (!response.ok) {
            throw new Error(`workspace counts ${String(response.status)}`);
          }
          const data: unknown = await response.json();
          if (
            data === null ||
            typeof data !== "object" ||
            !("items" in data) ||
            !Array.isArray(data.items)
          ) {
            throw new Error("Invalid workspace counts response");
          }
          const items = data.items.map((item: unknown) => {
            if (
              item === null ||
              typeof item !== "object" ||
              !("id" in item) ||
              typeof item.id !== "string" ||
              !("documentCount" in item) ||
              typeof item.documentCount !== "number"
            ) {
              throw new Error("Invalid workspace count item");
            }
            return { ...item, id: item.id, documentCount: item.documentCount };
          });
          return { ...data, items };
        },
      });
      await client.query({
        queryKey: ["projects", workspaceId],
        staleTime: 30000,
        queryFn: async () => {
          const response = await fetch(`/api/v1/workspaces/${workspaceId}/projects`);
          if (!response.ok) {
            throw new Error(`project counts ${String(response.status)}`);
          }
          const data: unknown = await response.json();
          return data;
        },
      });
      const required6 = counts.items.find((item) => item.id === workspaceId);
      if (required6 === undefined) {
        throw new Error(
          "Missing fixture value: counts.items.find((item) => item.id === workspaceId)",
        );
      }
      return required6.documentCount;
    },
    { workspaceId: ws, refresh },
  );
}

type CountObservation = {
  ms: number;
  target: "workspaces" | "projects";
  event: string;
  status?: number | string;
  fetchStatus?: string;
  isInvalidated?: boolean;
  count?: number | null;
  failure?: string;
};
type CountObservationWindow = Window & {
  __entitiesCountObservation?: { events: CountObservation[]; stop: () => void };
};

// Passive evidence for the unresolved CI timeout; never refetch or change query options.
async function observeCounts(page: Page, workspaceId: string) {
  await page.evaluate((ws) => {
    type Client = import("@tanstack/vue-query").QueryClient;
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: { _context: { provides: Record<string, Client> } };
    };
    const client = root.__vue_app__._context.provides.VUE_QUERY_CLIENT;
    if (!client) throw new Error("Missing app QueryClient for count observation");
    const events: CountObservation[] = [];
    const start = performance.now();
    const stop = client.getQueryCache().subscribe((event) => {
      if (event.type !== "updated") return;
      const rawKey: unknown = event.query.queryKey;
      if (!Array.isArray(rawKey)) return;
      const key: unknown[] = rawKey;
      const target =
        key.length === 2 && key[0] === "me" && key[1] === "workspaces"
          ? "workspaces"
          : key.length === 2 && key[0] === "projects" && key[1] === ws
            ? "projects"
            : null;
      if (!target) return;
      const state = event.query.state;
      const data: unknown = state.data;
      let count: number | null = null;
      if (data && typeof data === "object" && "items" in data && Array.isArray(data.items)) {
        const items: unknown[] = data.items;
        const item = items.find(
          (item) => item && typeof item === "object" && "id" in item && item.id === ws,
        );
        if (item && typeof item === "object" && "documentCount" in item) {
          if (typeof item.documentCount === "number") count = item.documentCount;
        }
      }
      events.push({
        ms: Math.round(performance.now() - start),
        target,
        event: event.action.type,
        status: state.status,
        fetchStatus: state.fetchStatus,
        isInvalidated: state.isInvalidated,
        ...(target === "workspaces" ? { count } : {}),
      });
      if (events.length > 80) events.shift();
    });
    (window as CountObservationWindow).__entitiesCountObservation = { events, stop };
  }, workspaceId);
  const network: CountObservation[] = [];
  const start = Date.now();
  const failureCodes = new Set([
    "net::ERR_ABORTED",
    "net::ERR_NETWORK_CHANGED",
    "net::ERR_CONNECTION_CLOSED",
    "net::ERR_CONNECTION_RESET",
    "net::ERR_CONNECTION_REFUSED",
    "net::ERR_CONNECTION_TIMED_OUT",
    "net::ERR_TIMED_OUT",
    "net::ERR_INTERNET_DISCONNECTED",
    "net::ERR_FAILED",
  ]);
  function record(request: Request, event: string, status?: number) {
    if (request.method() !== "GET") return;
    const path = new URL(request.url()).pathname;
    const target =
      path === "/api/v1/me/workspaces"
        ? "workspaces"
        : path === `/api/v1/workspaces/${workspaceId}/projects`
          ? "projects"
          : null;
    if (!target) return;
    const failure = event === "failed" ? request.failure()?.errorText : undefined;
    network.push({
      ms: Date.now() - start,
      target,
      event,
      ...(status === undefined ? {} : { status }),
      ...(failure === undefined ? {} : { failure: failureCodes.has(failure) ? failure : "other" }),
    });
    if (network.length > 80) network.shift();
  }
  const requested = (request: Request) => {
    record(request, "request");
  };
  const responded = (response: Response) => {
    record(response.request(), "response", response.status());
  };
  const finished = (request: Request) => {
    record(request, "finished");
  };
  const failed = (request: Request) => {
    record(request, "failed");
  };
  page.on("request", requested);
  page.on("response", responded);
  page.on("requestfinished", finished);
  page.on("requestfailed", failed);
  return {
    async report(host: "wiki" | "project") {
      const cache = await page.evaluate(
        () => (window as CountObservationWindow).__entitiesCountObservation?.events ?? [],
      );
      console.log("entities count observation", JSON.stringify({ host, cache, network }));
    },
    async stop() {
      page.off("request", requested);
      page.off("response", responded);
      page.off("requestfinished", finished);
      page.off("requestfailed", failed);
      await page.evaluate(() => {
        const fixture = window as CountObservationWindow;
        fixture.__entitiesCountObservation?.stop();
        delete fixture.__entitiesCountObservation;
      });
    },
  };
}

async function pushDocument(page: Page, path: string) {
  await page.evaluate(async (next) => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: {
        config: {
          globalProperties: {
            $router: {
              push: (path: string) => Promise<unknown>;
            };
          };
        };
      };
    };
    await root.__vue_app__.config.globalProperties.$router.push(next);
  }, path);
  await expect(page).toHaveURL(new RegExp(`${path}$`));
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15000 });
  await expect(editorOf(page)).toBeVisible();
}

for (const host of ["wiki", "project"] as const) {
  test(`${host}: real delayed move/trash survives same-room ACKs and retires on a room switch`, async ({
    browser,
    baseURL,
  }) => {
    const signed = await newSignedInPage(browser, baseURL, admin);
    const peer = await newSignedInPage(browser, baseURL, admin);
    const page = signed.page;
    const held: {
      release: () => void;
    }[] = [];
    let countObservation: Awaited<ReturnType<typeof observeCounts>> | undefined;
    try {
      const key = host === "wiki" ? "LCW" : "LCP";
      const f = await fixtures(page.request, key);
      async function extra(title: string) {
        if (host === "wiki") {
          return createDoc(page.request, f.ws, title);
        }
        const response = await page.request.post(
          `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents`,
          {
            data: { title, parentId: f.project.rootDocumentId },
          },
        );
        expect(response.status(), await response.text()).toBe(201);
        const document = (await readJson(response, flowSchemas.document)) as Item;
        return { ...document, path: `/w/${admin.workspaceSlug}/${key}-${String(document.number)}` };
      }
      const parent = await extra(`${key} parent`);
      const retired = await extra(`${key} retired parent`);
      const next = await extra(`${key} next room`);
      const original =
        host === "wiki"
          ? f.wiki
          : {
              ...f.document,
              path: `/w/${admin.workspaceSlug}/${key}-${String(f.document.number)}`,
            };
      const prefix =
        host === "wiki"
          ? `/api/v1/workspaces/${f.ws}/documents`
          : `/api/v1/workspaces/${f.ws}/projects/${f.project.id}/documents`;
      await openDoc(page, original.path);
      const countBefore = await retainedCounts(page, f.ws);
      countObservation = await observeCounts(page, f.ws);
      let workspaceRefreshes = 0;
      let projectRefreshes = 0;
      page.on("request", (request) => {
        if (request.method() !== "GET") {
          return;
        }
        if (request.url().endsWith("/api/v1/me/workspaces")) {
          workspaceRefreshes += 1;
        }
        if (request.url().endsWith(`/api/v1/workspaces/${f.ws}/projects`)) {
          projectRefreshes += 1;
        }
      });
      await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
      const parentSelect = page.getByLabel("새 위치(부모 문서)");
      await parentSelect.selectOption(parent.id);
      const moved = await holdResponse(page, `${prefix}/${original.id}/move`);
      held.push(moved);
      await page.getByRole("button", { name: "이동", exact: true }).click();
      expect(await moved.response).toBe(200);
      await openDoc(peer.page, original.path);
      await editorOf(page).click();
      await page.keyboard.type(`${key} edit during move`);
      await save(page);
      await expect(editorOf(peer.page)).toContainText(`${key} edit during move`);
      moved.release();
      await expect(parentSelect).toHaveValue("");
      const movedMeta = await page.request.get(`${prefix}/${original.id}`);
      expect((await readJson(movedMeta, flowSchemas.document)).parentId).toBe(parent.id);
      // Both real queries already have fresh 30-second cache data. The
      // active workspace query refetches; inactive project data becomes stale.
      expect(workspaceRefreshes).toBeGreaterThan(0);
      const projectInvalidated = await page.evaluate((workspaceId) => {
        type Client = import("@tanstack/vue-query").QueryClient;
        const root = document.getElementById("root") as HTMLElement & {
          __vue_app__: {
            _context: {
              provides: Record<string, Client>;
            };
          };
        };
        const required7 = root.__vue_app__._context.provides.VUE_QUERY_CLIENT;
        if (required7 === undefined) {
          throw new Error(
            "Missing fixture value: root.__vue_app__._context.provides.VUE_QUERY_CLIENT",
          );
        }
        return required7.getQueryState(["projects", workspaceId])?.isInvalidated;
      }, f.ws);
      expect(projectRefreshes > 0 || projectInvalidated).toBe(true);
      expect(await retainedCounts(page, f.ws)).toBe(countBefore);

      // A real missing-parent failure still appears after a successful persist
      // ACK. The browser retains the option; the API independently trashes it.
      await parentSelect.selectOption(retired.id);
      expect((await page.request.post(`${prefix}/${retired.id}/trash`)).ok()).toBe(true);
      const failed = await holdResponse(page, `${prefix}/${original.id}/move`);
      held.push(failed);
      await page.getByRole("button", { name: "이동", exact: true }).click();
      expect(await failed.response).toBeGreaterThanOrEqual(400);
      await editorOf(page).click();
      await page.keyboard.press("End");
      await page.keyboard.type(" error ACK");
      await save(page);
      failed.release();
      await expect(page.locator(".document-page__error[role=alert]")).toBeVisible();

      // Retire a real in-flight error by switching via the actual SPA router.
      const lateError = await holdResponse(page, `${prefix}/${original.id}/move`);
      held.push(lateError);
      await page.getByRole("button", { name: "이동", exact: true }).click();
      expect(await lateError.response).toBeGreaterThanOrEqual(400);
      await pushDocument(page, next.path);
      const deliveredError = page.waitForResponse((response) =>
        response.url().endsWith(`${prefix}/${original.id}/move`),
      );
      lateError.release();
      await deliveredError;
      await expect(page.locator(".document-page__error[role=alert]")).toHaveCount(0);

      // Retire successful trash: cache effects still apply to the old scope,
      // but its completion must never redirect the newly opened room.
      await pushDocument(page, original.path);
      // The earlier API-only fixture deletion bypassed browser invalidation.
      // Refresh the real baseline once, then retain it through the room switch.
      const beforeTrash = await retainedCounts(page, f.ws, true);
      const lateTrash = await holdResponse(page, `${prefix}/${original.id}/trash`);
      held.push(lateTrash);
      page.once("dialog", (dialog) => dialog.accept());
      await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
      await page.getByRole("button", { name: "휴지통으로 이동", exact: true }).click();
      expect(await lateTrash.response).toBe(200);
      await pushDocument(page, next.path);
      const deliveredTrash = page.waitForResponse((response) =>
        response.url().endsWith(`${prefix}/${original.id}/trash`),
      );
      lateTrash.release();
      await deliveredTrash;
      await expect.poll(() => retainedCounts(page, f.ws)).toBe(beforeTrash - 1);
      await expect(page).toHaveURL(new RegExp(`${next.path}$`));
      await expect(page.getByLabel("문서 제목")).toHaveValue(`${key} next room`);

      // Current-room trash gets a persist ACK before delivery and must navigate.
      await openDoc(peer.page, next.path);
      const currentTrash = await holdResponse(page, `${prefix}/${next.id}/trash`, true);
      held.push(currentTrash);
      // Deleting a document may close its collab room. Hold the outgoing
      // request until the captured operation has observed a real persist ACK.
      page.once("dialog", (dialog) => dialog.accept());
      await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
      await page.getByRole("button", { name: "휴지통으로 이동", exact: true }).click();
      await editorOf(page).click();
      await page.keyboard.type("during trash ACK");
      await save(page);
      await expect(editorOf(peer.page)).toContainText("during trash ACK");
      currentTrash.forward();
      expect(await currentTrash.response).toBe(200);
      currentTrash.release();
      await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/trash$`));
      const counts = await page.request.get("/api/v1/me/workspaces");
      const fixtureValue2 = (await readJson(counts, flowSchemas.workspaces)).items.find(
        (item: { id: string }) => item.id === f.ws,
      );
      if (fixtureValue2 === undefined)
        throw new Error(
          "Missing fixture value: (await readJson(counts, flowSchemas.workspaces)).items.find(\n          (item: { id: string }) => item.id === f.ws,\n        )",
        );
      expect(fixtureValue2.documentCount).toBe(beforeTrash - 2);
    } catch (error) {
      if (countObservation) {
        try {
          await countObservation.report(host);
        } catch {
          console.log("entities count observation unavailable", host);
        }
      }
      throw error;
    } finally {
      try {
        await countObservation?.stop();
      } catch {
        console.log("entities count observation cleanup unavailable", host);
      }
      for (const response of held) response.release();
      await peer.context.close();
      await signed.context.close();
    }
  });
}
