import { randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createListener } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { expect, test as base, type Browser, type Page } from "@playwright/test";
import { createServer, type ViteDevServer } from "vite";
import { z } from "zod";
import { login, readJson, flowSchemas } from "./helpers";
import { admin } from "./workspace-wiki-vue-editor";
import {
  ownedServerChildEnv,
  processGroupMembers,
  readProcMember,
  signalOwnedGroup,
  type ProcMember,
} from "../e2e-pending/collab-restart";

// The normal E2E group supplies native binaries and an isolated app-role DB.
// Own a Rust process with the actual dev public origin; never rewrite Origin.
const webRoot = path.resolve(import.meta.dirname, "..");
let devURL: string;

type DevRuntime = { start: (phase: string) => Promise<void>; stop: () => Promise<void> };
const test = base.extend<object, { devRuntime: DevRuntime }>({
  devRuntime: [
    // Playwright requires fixture dependencies as an object destructuring pattern.
    // This worker fixture has no fixture dependencies.
    // eslint-disable-next-line no-empty-pattern
    async ({}, use, workerInfo) => {
      const ownedDir = mkdtempSync(path.join(tmpdir(), "fvoci-tb-a-dev-"));
      const cacheDir = path.join(ownedDir, "vite-cache");
      let server: ViteDevServer | undefined;
      let child: ChildProcess | undefined;
      let identity: ProcMember | null = null;
      const stop = async () => {
        let closeError: unknown;
        try {
          await server?.close();
        } catch (error) {
          closeError = error;
        }
        server = undefined;
        if (child?.pid) {
          const pid = child.pid;
          const owners = processGroupMembers(pid);
          if (child.exitCode === null && child.signalCode === null) {
            const exited = once(child, "exit");
            if (readProcMember(pid)?.starttime !== identity?.starttime) {
              throw new Error("Rust process ownership changed before cleanup");
            }
            // Let Rust flush and reap children before checking the whole group.
            process.kill(pid, "SIGTERM");
            await Promise.race([exited, delay(5_000)]);
          }
          const deadline = Date.now() + 5_000;
          while (processGroupMembers(pid).length && Date.now() < deadline) await delay(50);
          if (processGroupMembers(pid).length) {
            signalOwnedGroup(pid, owners);
            throw new Error("Rust graceful shutdown left process group members");
          }
        }
        child = undefined;
        if (closeError instanceof Error) throw closeError;
        if (closeError !== undefined) throw new Error("Vite close failed", { cause: closeError });
      };
      try {
        await use({
          stop,
          async start(phase) {
            // Reserve via port 0, then bind Vite strictly to that selected port.
            // Any intervening collision fails the test instead of silently
            // changing the public origin or retrying on another port.
            const listener = createListener();
            listener.listen(0, "127.0.0.1");
            await once(listener, "listening");
            const address = listener.address();
            if (!address || typeof address === "string") throw new Error("port allocation failed");
            const port = address.port;
            await new Promise<void>((resolve, reject) => {
              listener.close((error) => {
                if (error) reject(error);
                else resolve();
              });
            });
            devURL = `http://127.0.0.1:${String(port)}`;
            const binary = process.env.FVOCI_E2E_SERVER_BIN;
            if (!binary) throw new Error("FVOCI_E2E_SERVER_BIN missing");
            if (!process.env.DATABASE_APP_URL) throw new Error("isolated app DB URL missing");
            const runtimeEnv = ownedServerChildEnv("127.0.0.1:0");
            const logPath = path.join(workerInfo.project.outputDir, `tb-a-${phase}`, "server.log");
            mkdirSync(path.dirname(logPath), { recursive: true });
            const childEnv = {
              ...runtimeEnv,
              FVOCI_BIND: "127.0.0.1:0",
              FVOCI_PUBLIC_ORIGIN: devURL,
              FVOCI_STATIC_DIR: path.join(webRoot, "dist"),
              FVOCI_STORAGE_DIR: ownedDir,
              RUST_LOG: "warn,tower_http=debug",
            };
            child = spawn(binary, [], {
              detached: true,
              env: childEnv,
              stdio: ["ignore", "pipe", "pipe"],
            });
            if (!child.pid) throw new Error("Rust spawn returned no PID");
            identity = readProcMember(child.pid);
            const backend = await new Promise<string>((resolve, reject) => {
              const timer = setTimeout(() => {
                reject(new Error("Rust startup deadline"));
              }, 30_000);
              let output = "";
              const capture = (bytes: Buffer) => {
                appendFileSync(logPath, bytes);
                output = (output + bytes.toString()).slice(-8192);
                const url = output.match(/fvoci-server listening on (http:\/\/[^\s]+)/)?.[1];
                if (url) {
                  clearTimeout(timer);
                  resolve(url);
                }
              };
              child?.stdout?.on("data", capture);
              child?.stderr?.on("data", capture);
              child?.once("error", (error) => {
                clearTimeout(timer);
                reject(error);
              });
              child?.once("exit", (code) => {
                clearTimeout(timer);
                reject(new Error(`Rust exited before readiness: ${String(code)}`));
              });
            });
            const previousProxy = process.env.API_PROXY_TARGET;
            const cacheWasPresent = existsSync(path.join(cacheDir, "deps/_metadata.json"));
            try {
              process.env.API_PROXY_TARGET = backend;
              server = await createServer({
                root: webRoot,
                configFile: path.join(webRoot, "vite.config.ts"),
                cacheDir,
                plugins: [
                  {
                    name: "tb-a-proxy-evidence",
                    configResolved(config) {
                      for (const [route, option] of Object.entries(config.server.proxy ?? {})) {
                        const proxy = typeof option === "string" ? { target: option } : option;
                        const configure = proxy.configure;
                        proxy.configure = (instance, options) => {
                          configure?.(instance, options);
                          const record = (
                            event: string,
                            requestPath?: string,
                            status?: number,
                            error?: string,
                          ) => {
                            appendFileSync(
                              logPath,
                              JSON.stringify({
                                event,
                                route,
                                target: proxy.target,
                                path: requestPath?.split("?")[0],
                                status,
                                error,
                              }) + "\n",
                            );
                          };
                          record("proxy-configured");
                          instance.on("proxyReq", (_outgoing, request) => {
                            record("proxy-request", request.url);
                          });
                          instance.on("proxyRes", (response, request) => {
                            record("proxy-response", request.url, response.statusCode);
                          });
                          instance.on("error", (error, request) => {
                            record("proxy-error", request.url, undefined, error.message);
                          });
                        };
                        if (config.server.proxy) config.server.proxy[route] = proxy;
                      }
                    },
                  },
                ],
                server: { host: "127.0.0.1", port, strictPort: true },
              });
            } finally {
              if (previousProxy === undefined) delete process.env.API_PROXY_TARGET;
              else process.env.API_PROXY_TARGET = previousProxy;
            }
            await server.listen();
            writeFileSync(
              path.join(workerInfo.project.outputDir, `tb-a-${phase}-runtime.json`),
              JSON.stringify({
                phase,
                workerIndex: workerInfo.workerIndex,
                bun: process.versions.bun,
                devURL,
                backend,
                cacheDir,
                cacheWasPresent,
                pid: child.pid,
                executableCommand: binary,
                spawnAppRole: new URL(process.env.DATABASE_APP_URL).username,
                spawnEnvOwnerCredentialsPresent: [
                  "DATABASE_URL",
                  "FVOCI_MIGRATION_URL",
                  "FVOCI_E2E_ADMIN_DATABASE_URL",
                  "TEST_DATABASE_URL",
                ].some((key) => key in childEnv),
                publicOrigin: childEnv.FVOCI_PUBLIC_ORIGIN,
                proxyEnvRestored: process.env.API_PROXY_TARGET === previousProxy,
                resolvedProxy: Object.fromEntries(
                  Object.entries(server.config.server.proxy ?? {}).map(([route, option]) => [
                    route,
                    typeof option === "string" ? option : option.target,
                  ]),
                ),
                watchDisabled: server.config.server.watch === null,
                polling: process.env.CHOKIDAR_USEPOLLING === "1",
              }),
            );
          },
        });
      } finally {
        try {
          await stop();
        } finally {
          rmSync(ownedDir, { recursive: true, force: true });
        }
      }
    },
    { scope: "worker" },
  ],
});

async function attachJson(name: string, value: unknown) {
  const file = test.info().outputPath(`${name}.json`);
  writeFileSync(file, JSON.stringify(value, null, 2));
  await test.info().attach(name, { path: file, contentType: "application/json" });
}

async function setupDevelopmentPage(page: Page) {
  const events: object[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith("/api/v1/"))
      events.push({ at: Date.now(), event: "request", path: url.pathname });
  });
  page.on("response", (response) => {
    const url = new URL(response.url());
    if (url.pathname.startsWith("/api/v1/"))
      events.push({
        at: Date.now(),
        event: "response",
        path: url.pathname,
        status: response.status(),
      });
  });
  page.on("requestfailed", (request) =>
    events.push({
      at: Date.now(),
      event: "failed",
      path: new URL(request.url()).pathname,
      error: request.failure()?.errorText,
    }),
  );
  try {
    // A cold dev bootstrap can still be fetching when the document loads.
    // Await actual Rust responses before asserting the setup/login UI.
    const bootstrap = Promise.all(
      ["/api/v1/setup", "/api/v1/auth/me"].map((pathname) =>
        page.waitForResponse(
          (response) =>
            new URL(response.url()).pathname === pathname && response.request().method() === "GET",
        ),
      ),
    );
    const [, [setup, me]] = await Promise.all([
      page.goto("/").then(async () => {
        events.push({
          at: Date.now(),
          event: "document-loaded",
          loadingVisible: await page.getByRole("status").isVisible(),
        });
      }),
      bootstrap,
    ]);
    events.push({ at: Date.now(), event: "bootstrap-ready" });
    expect(setup?.status()).toBe(200);
    expect(me?.status()).toBe(401);
    await expect(
      page
        .getByRole("button", { name: "시작하기" })
        .or(page.getByRole("button", { name: "로그아웃" }))
        .or(page.getByRole("button", { name: "로그인", exact: true })),
    ).toBeVisible();
    if (await page.getByRole("button", { name: "시작하기" }).count()) {
      await page.getByLabel("성").fill(admin.familyName);
      await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
      await page.getByLabel("이메일").fill(admin.email);
      await page.getByLabel("비밀번호").fill(admin.password);
      await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
      await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
      await page.getByRole("button", { name: "시작하기" }).click();
    } else if (
      page.url().includes("/login") ||
      (await page.getByRole("button", { name: "로그인", exact: true }).count())
    ) {
      await login(page, admin.email, admin.password);
    }
    await expect(page).toHaveURL(/\/$/);
    await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
    events.push({ at: Date.now(), event: "authenticated-ui-ready" });
  } finally {
    await attachJson("setup-events", events);
  }
}

async function signedIn(browser: Browser) {
  const context = await browser.newContext({ baseURL: devURL });
  const page = await context.newPage();
  await login(page, admin.email, admin.password);
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
  return { context, page };
}

async function workspaceId(page: Page): Promise<string> {
  const response = await page.request.get("/api/v1/me/workspaces");
  expect(response.ok()).toBe(true);
  const workspace = (await readJson(response, flowSchemas.workspaces)).items.find(
    (item) => item.slug === admin.workspaceSlug,
  );
  if (!workspace) throw new Error("TB-A workspace missing");
  return workspace.id;
}

const resourceSchema = z.object({ id: z.string(), number: z.number() });
const projectSchema = z.object({ id: z.string() });
const bodySchema = z.object({ contentJson: z.unknown() });

async function editAndReadBack(
  browser: Browser,
  page: Page,
  resource: { path: string; readback: string; task: boolean },
) {
  const errors: string[] = [];
  const frames: { direction: string; message: string }[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("websocket", (socket) => {
    const record =
      (direction: string) =>
      ({ payload }: { payload: string | Buffer }) => {
        const message = payload.toString().match(/persist(?:ed)?:[\w-]+/)?.[0];
        if (message) frames.push({ direction, message });
      };
    socket.on("framesent", record("framesent"));
    socket.on("framereceived", record("framereceived"));
  });
  await page.goto(resource.path);
  const scope = resource.task ? page.getByTestId("task-body") : page.locator(".document-page");
  const editor = scope.locator(".fvoci-editor .ProseMirror").first();
  await expect(editor).toBeVisible();
  await expect(editor).toHaveAttribute("contenteditable", "true");
  await expect(scope.locator('[data-collab-status="connected"]')).toBeVisible();
  const marker = `TB-A-${randomUUID()}`;
  for (const text of [marker, " newest-edit"]) {
    await editor.click();
    await page.keyboard.press("ControlOrMeta+End");
    await page.keyboard.type(text);
    await expect(scope.locator('[data-collab-persisted="false"]')).toBeVisible();
    const sentBefore = frames.filter((frame) => frame.direction === "framesent").length;
    await scope.getByRole("button", { name: "저장", exact: true }).first().click();
    await expect(scope.locator('[data-collab-persisted="true"]')).toBeVisible();
    const sent = frames.filter((frame) => frame.direction === "framesent");
    expect(sent.length).toBeGreaterThan(sentBefore);
    const request = sent.at(-1);
    if (!request) throw new Error("persist request missing");
    expect(frames).toContainEqual({
      direction: "framereceived",
      message: request.message.replace("persist:", "persisted:"),
    });
  }
  const expected = `${marker} newest-edit`;
  await expect(editor).toContainText(expected);
  // Close the editing page before opening a client with no shared Query/Y.Doc,
  // browser storage or session cookies. Its own login and GET are the oracle.
  await page.close();
  const fresh = await signedIn(browser);
  try {
    const readback = await fresh.page.request.get(resource.readback);
    expect(readback.ok()).toBe(true);
    const saved = bodySchema.parse(await readback.json());
    expect(JSON.stringify(saved.contentJson)).toContain(expected);
    await fresh.page.goto(resource.path);
    await expect(fresh.page.locator(".fvoci-editor .ProseMirror").first()).toContainText(expected);
    await fresh.page.goto(`/w/${admin.workspaceSlug}`);
    await fresh.page.goto(resource.path);
    await expect(fresh.page.locator(".fvoci-editor .ProseMirror").first()).toContainText(expected);
    // An unauthenticated independent request cannot read either persisted body.
    const anonymous = await browser.newContext({ baseURL: devURL });
    try {
      expect((await anonymous.request.get(resource.readback)).status()).toBe(401);
    } finally {
      await anonymous.close();
    }
    expect(errors).toEqual([]);
    await attachJson("persist-and-readback", { resource, expected, frames, saved });
  } finally {
    await fresh.context.close();
  }
}

for (const phase of ["cold", "restart"] as const) {
  test.describe(`TB-A ${phase} development server`, () => {
    test.beforeAll(async ({ browser, devRuntime }) => {
      await devRuntime.start(phase);
      const context = await browser.newContext({ baseURL: devURL });
      try {
        await setupDevelopmentPage(await context.newPage());
      } finally {
        await context.close();
      }
    });

    test.afterAll(async ({ devRuntime }) => {
      await devRuntime.stop();
    });

    test("editor UUID and web Zod 3 retain their runtime contracts", async ({ browser }) => {
      const context = await browser.newContext({ baseURL: devURL });
      try {
        const page = await context.newPage();
        await page.goto("/login");
        await expect(page.getByLabel("이메일")).toBeVisible();
        const uuidPath = `/@fs${path.resolve(webRoot, "../../packages/editor/src/uuid.ts")}`;
        const uuidResponse = await page.request.get(uuidPath);
        expect(uuidResponse.ok()).toBe(true);
        const uuidBytes = await uuidResponse.text();
        const editorZodURL = uuidBytes.match(/from\s+["']([^"']+)["']/)?.[1];
        if (!editorZodURL) throw new Error("transformed editor Zod import missing");
        const webResponse = await page.request.get("/src/lib/validators.ts");
        expect(webResponse.ok()).toBe(true);
        const webBytes = await webResponse.text();
        const webZodURL = webBytes.match(/from\s+["']([^"']*zod[^"']*)["']/)?.[1];
        if (!webZodURL) throw new Error("transformed web Zod import missing");
        const contracts = await page.evaluate(
          async ({ uuidPath, editorZodURL, webZodURL }) => {
            type ZodModule = { z: { uuid?: () => unknown; string: () => unknown } };
            type UuidModule = { uuid: { safeParse: (value: string) => { success: boolean } } };
            const uuid = (await import(uuidPath)) as UuidModule;
            const editor = (await import(editorZodURL)) as ZodModule;
            const web = (await import(webZodURL)) as ZodModule;
            return {
              editorUuid: typeof editor.z.uuid,
              webUuid: typeof web.z.uuid,
              webString: typeof web.z.string,
              accepted: uuid.uuid.safeParse("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").success,
              rejected: uuid.uuid.safeParse("aaaaaaaa-aaaa-4aaa-7aaa-aaaaaaaaaaaa").success,
            };
          },
          { uuidPath, editorZodURL, webZodURL },
        );
        expect(contracts).toEqual({
          editorUuid: "function",
          webUuid: "undefined",
          webString: "function",
          accepted: true,
          rejected: false,
        });
        await attachJson("transformed-zod-imports", {
          editorZodURL,
          webZodURL,
          uuidBytes,
          webBytes,
          contracts,
        });
      } finally {
        await context.close();
      }
    });

    test("task editor saves newest body and a new client reads it", async ({ browser }) => {
      const current = await signedIn(browser);
      try {
        const ws = await workspaceId(current.page);
        const key = phase === "cold" ? "TAC" : "TAR";
        const projectResponse = await current.page.request.post(
          `/api/v1/workspaces/${ws}/projects`,
          {
            data: { key, name: `TB-A ${phase}`, visibility: "workspace" },
          },
        );
        expect(projectResponse.status()).toBe(201);
        const project = projectSchema.parse(await projectResponse.json());
        const response = await current.page.request.post(
          `/api/v1/workspaces/${ws}/projects/${project.id}/tasks`,
          { data: { title: "TB-A task" } },
        );
        expect(response.status()).toBe(201);
        const task = resourceSchema.parse(await response.json());
        await editAndReadBack(browser, current.page, {
          path: `/w/${admin.workspaceSlug}/${key}-${String(task.number)}`,
          readback: `/api/v1/workspaces/${ws}/tasks/${task.id}`,
          task: true,
        });
      } finally {
        await current.context.close();
      }
    });

    test("wiki editor saves newest body and a new client reads it", async ({ browser }) => {
      const current = await signedIn(browser);
      try {
        const ws = await workspaceId(current.page);
        const response = await current.page.request.post(`/api/v1/workspaces/${ws}/documents`, {
          data: { parentId: null, title: `TB-A wiki ${phase}` },
        });
        expect(response.status()).toBe(201);
        const doc = resourceSchema.parse(await response.json());
        await editAndReadBack(browser, current.page, {
          path: `/w/${admin.workspaceSlug}/WIKI-${String(doc.number)}`,
          readback: `/api/v1/workspaces/${ws}/documents/${doc.id}/body`,
          task: false,
        });
      } finally {
        await current.context.close();
      }
    });
  });
}
