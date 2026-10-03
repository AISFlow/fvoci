import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import path from "node:path";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { expect, test, type Page } from "@playwright/test";
import { z } from "zod";

import { ZOTERO_FIXTURE_KEY as key, expectConnectedSourceObserver } from "./zotero-archive-oracles";
const root = path.resolve(import.meta.dirname, "../../..");
const workspaceSchema = z.object({ id: z.string().uuid(), slug: z.string() });
const meSchema = z.object({ userId: z.string().uuid() }).passthrough();
const referenceSchema = z
  .object({
    id: z.string().uuid(),
    itemKey: z.string(),
    documentDisplayId: z.string(),
    bibliography: z.object({ title: z.string() }).passthrough(),
    availability: z.string(),
    links: z.array(
      z.object({ taskId: z.string().nullable(), displayId: z.string() }).passthrough(),
    ),
  })
  .passthrough();
const librarySchema = z.object({
  connector: z
    .object({ id: z.string().uuid(), completedVersion: z.string(), state: z.string() })
    .passthrough(),
  references: z.array(referenceSchema),
  collections: z.array(
    z.object({ key: z.string(), parentKey: z.string().nullable() }).passthrough(),
  ),
});
const inputSchema = z.object({
  taskId: z.string().uuid(),
  taskDisplayId: z.string(),
  documentId: z.string().uuid(),
});
class Fixture {
  private readonly pending: Array<{
    resolve: (value: unknown) => void;
    reject: (error: Error) => void;
  }> = [];
  private readonly queued: unknown[] = [];
  readonly child: ChildProcessWithoutNullStreams;
  readonly exited: Promise<number | null>;
  constructor() {
    const target = process.env.CARGO_TARGET_DIR ?? path.join(root, "target");
    const profile = process.env.FVOCI_E2E_PROFILE ?? "debug";
    // Only the restricted app URL enters this fixture; no inherited owner DB,
    // cloud credentials, maintenance keyrings, or production connector config.
    this.child = spawn(path.join(target, profile, "fvoci-e2e-fixture"), ["zotero-readonly"], {
      env: {
        PATH: process.env.PATH,
        DATABASE_APP_URL: process.env.DATABASE_APP_URL,
        FVOCI_COLLAB_ENGINE: process.env.FVOCI_COLLAB_ENGINE,
        FVOCI_E2E_SERVER_BIN: path.join(target, profile, "fvoci-server"),
        FVOCI_E2E_DIST: path.join(root, "apps/web/dist"),
        FVOCI_MEILI_URL: process.env.FVOCI_MEILI_URL,
        FVOCI_MEILI_KEY: process.env.FVOCI_MEILI_KEY,
        TMPDIR: process.env.TMPDIR,
      },
      stdio: "pipe",
    });
    createInterface({ input: this.child.stdout }).on("line", (line) => {
      const value: unknown = JSON.parse(line);
      const waiter = this.pending.shift();
      if (waiter) waiter.resolve(value);
      else this.queued.push(value);
    });
    this.child.stderr.on("data", () => {
      /* no credential-bearing raw child output */
    });
    this.child.on("error", () => {
      for (const waiter of this.pending.splice(0))
        waiter.reject(new Error("Zotero fixture process failed"));
    });
    this.exited = new Promise((resolve) =>
      this.child.on("exit", (code) => {
        for (const waiter of this.pending.splice(0))
          waiter.reject(new Error("Zotero fixture exited before acknowledgement"));
        resolve(code);
      }),
    );
  }
  next(): Promise<unknown> {
    if (this.queued.length) return Promise.resolve(this.queued.shift());
    return new Promise((resolve, reject) => this.pending.push({ resolve, reject }));
  }
  async command(value: Record<string, unknown>): Promise<unknown> {
    this.child.stdin.write(`${JSON.stringify(value)}\n`);
    return this.next();
  }
  async stop(): Promise<void> {
    const value = await this.command({ command: "stop" });
    expect(value).toEqual({ stopped: true, ownedResources: 0 });
    expect(await this.exited).toBe(0);
  }
}
async function setup(page: Page): Promise<void> {
  await page.goto("/setup");
  await expect(page.getByRole("heading", { name: "초기 설정", exact: true })).toBeVisible();
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("연구");
  await page.getByLabel("이메일").fill("Admin@Example.COM");
  await page.getByLabel("비밀번호").fill("supersecret1");
  await page.getByLabel("워크스페이스 이름").fill("Zotero fixture team");
  await page.getByLabel("주소(영문)").fill("zotero-tracer");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
}

test("Zotero read-only UI commits private metadata and links visible to a new client", async ({
  browser,
}, info) => {
  const fixture = new Fixture();
  const { origin } = z.object({ origin: z.string().url() }).parse(await fixture.next());
  const context = await browser.newContext({ baseURL: origin });
  const page = await context.newPage();
  const assetReads: Array<
    Promise<{ asset: string; sha256: string; matchesDist: boolean; credentialPresent: boolean }>
  > = [];
  function observeStaticAssets(observedPage: Page): typeof assetReads {
    const observed: typeof assetReads = [];
    observedPage.on("response", (response) => {
      const url = new URL(response.url());
      if (url.origin !== origin || !/^\/assets\/[A-Za-z0-9_.-]+\.(js|css)$/.test(url.pathname))
        return;
      const read = Promise.all([
        response.body(),
        readFile(path.join(root, "apps/web/dist", url.pathname.slice(1))),
      ])
        .then(([served, dist]) => ({
          asset: url.pathname,
          sha256: createHash("sha256").update(served).digest("hex"),
          matchesDist: served.equals(dist),
          credentialPresent: served.includes(Buffer.from(key)),
        }))
        .catch(() => ({
          asset: url.pathname,
          sha256: "unavailable",
          matchesDist: false,
          credentialPresent: false,
        }));
      assetReads.push(read);
      observed.push(read);
    });
    return observed;
  }
  let second: Awaited<ReturnType<typeof browser.newContext>> | undefined;
  try {
    await setup(page);
    const mainReads = observeStaticAssets(page);
    const workspaceResponse = await context.request.post("/api/v1/me/personal-workspace", {
      headers: { Origin: origin },
    });
    expect(workspaceResponse.status()).toBe(200);
    const workspace = workspaceSchema.parse(await workspaceResponse.json());
    const meResponse = await context.request.get("/api/v1/auth/me");
    expect(meResponse.status()).toBe(200);
    const me = meSchema.parse(await meResponse.json());
    const authoredResponse = await context.request.post(
      `/api/v1/workspaces/${workspace.id}/personal-input`,
      {
        headers: { Origin: origin },
        data: { requestId: randomUUID(), intent: "task", title: "Compare my authored evidence" },
      },
    );
    expect(authoredResponse.status()).toBe(201);
    const authored = inputSchema.parse(await authoredResponse.json());
    const authoredBody = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "authored-commentary" },
          content: [{ type: "text", text: "내가 쓴 의견과 비교 기록" }],
        },
      ],
    };
    const bodyWrite = await context.request.put(
      `/api/v1/workspaces/${workspace.id}/documents/${authored.documentId}/body`,
      {
        headers: { Origin: origin },
        data: { contentJson: authoredBody },
      },
    );
    expect(bodyWrite.status()).toBe(200);
    await page.goto("/settings/account");
    const section = page.getByTestId("zotero-section");
    await expect(section).toBeVisible();
    await section.getByLabel("Zotero 자료실 ID").fill("42");
    await section.getByLabel("Zotero 자료실 주소").fill("https://www.zotero.org/users/42");
    await section.getByLabel("읽기 전용 연동 키").fill(key);
    await section.getByRole("button", { name: "자료실 연결", exact: true }).click();
    await expect(section.getByLabel("읽기 전용 연동 키")).toHaveValue("");
    await section.getByRole("button", { name: "자료 가져오기", exact: true }).click();
    await expect(
      section.getByRole("heading", { name: "합성 연구 자료 🙂", exact: true }),
    ).toBeVisible();
    const row = section
      .locator("li[data-reference-id]")
      .filter({ has: page.getByRole("heading", { name: "합성 연구 자료 🙂", exact: true }) });
    await expect(row).toContainText("Study, Evidence");
    await expect(row.getByRole("link", { name: "Zotero 원자료 열기" })).toHaveAttribute(
      "href",
      "https://www.zotero.org/users/42/items/ABCD2345",
    );
    await row.getByRole("button", { name: "문서·할 일 연결" }).click();
    await section.getByLabel("연결할 대상").click();
    await page.getByRole("option", { name: "할 일", exact: true }).click();
    await section.getByLabel("문서·할 일 번호").fill(authored.taskDisplayId);
    await section.locator("form").last().getByRole("button", { name: "문서·할 일 연결" }).click();
    await expect(
      row.getByRole("link", { name: authored.taskDisplayId, exact: true }),
    ).toBeVisible();
    const connectors = z
      .object({ connectors: z.array(z.object({ id: z.string().uuid(), libraryType: z.string() })) })
      .parse(await (await context.request.get(`/api/v1/workspaces/${workspace.id}/zotero`)).json());
    const connector = connectors.connectors.find((item) => item.libraryType === "user");
    if (!connector) throw new Error("committed user library missing");
    const observer = expectConnectedSourceObserver(
      await fixture.command({
        command: "observe",
        userId: me.userId,
        workspaceId: workspace.id,
        connectorId: connector.id,
      }),
      connector.id,
    );
    expect(observer.rows).toHaveLength(2);
    const committed = observer.rows[0];
    if (!committed) throw new Error("missing committed reference");
    expect(committed).toEqual({
      id: committed.id,
      documentId: committed.id,
      itemKey: "ABCD2345",
      title: "합성 연구 자료 🙂",
      completedVersion: "12",
      availability: "available",
      edges: 1,
    });
    second = await browser.newContext({ baseURL: origin });
    await second.addCookies(await context.cookies());
    const newClient = librarySchema.parse(
      await (
        await second.request.get(
          `/api/v1/workspaces/${workspace.id}/zotero/libraries/${connector.id}`,
        )
      ).json(),
    );
    const newReference = newClient.references[0];
    if (!newReference) throw new Error("new client missing the committed reference");
    expect(newReference.id).toBe(committed.id);
    expect(newReference.links[0]?.taskId).toBe(authored.taskId);
    expect(newClient.collections[1]?.parentKey).toBe("BCDE3456");
    expect(JSON.stringify(newClient)).not.toContain(key);
    const indexed = z
      .object({ indexedWorkspaces: z.literal(1), pages: z.number().positive() })
      .parse(await fixture.command({ command: "index", workspaceId: workspace.id }));
    expect(indexed.indexedWorkspaces).toBe(1);
    const searchResponse = await second.request.get(`/api/v1/workspaces/${workspace.id}/search`, {
      params: { q: "9780000000000", type: "document" },
    });
    expect(searchResponse.status()).toBe(200);
    const results = z
      .object({ items: z.array(z.object({ id: z.string().uuid() }).passthrough()) })
      .parse(await searchResponse.json());
    expect(results.items.some((item) => item.id === committed.id)).toBe(true);
    const newPage = await second.newPage();
    const newClientReads = observeStaticAssets(newPage);
    await newPage.goto("/settings/account");
    await expect(
      newPage
        .getByTestId("zotero-section")
        .getByRole("heading", { name: "합성 연구 자료 🙂", exact: true }),
    ).toBeVisible();
    const newRow = newPage
      .getByTestId("zotero-section")
      .locator("li[data-reference-id]")
      .filter({ has: newPage.getByRole("heading", { name: "합성 연구 자료 🙂", exact: true }) });
    await newRow.getByRole("link", { name: "내 문서 열기", exact: true }).click();
    await expect(newPage).toHaveURL(`/w/${workspace.slug}/${newReference.documentDisplayId}`);
    await expect(newPage.getByLabel("문서 제목", { exact: true })).toHaveValue("Zotero reference");
    await expect(newPage.locator(".fvoci-editor .ProseMirror").first()).toHaveAttribute(
      "contenteditable",
      "true",
    );
    await newPage.goto("/settings/account");
    await newRow.getByRole("link", { name: authored.taskDisplayId, exact: true }).click();
    await expect(newPage).toHaveURL(`/w/${workspace.slug}/${authored.taskDisplayId}`);
    await expect(
      newPage.getByRole("heading", { level: 1, name: "Compare my authored evidence", exact: true }),
    ).toBeVisible();
    await expect(newPage.getByTestId("task-edit-title")).toHaveValue(
      "Compare my authored evidence",
    );
    await fixture.command({ command: "mode", mode: 2 });
    await section.getByRole("button", { name: "자료 가져오기", exact: true }).click();
    await expect(row).toContainText("Zotero에서 삭제됨");
    await expect(
      row.getByRole("link", { name: authored.taskDisplayId, exact: true }),
    ).toBeVisible();
    await section.getByRole("button", { name: "연결 끊기", exact: true }).click();
    await page.getByRole("dialog").getByRole("button", { name: "연결 끊기", exact: true }).click();
    await expect(section).toContainText("연결 끊김 — 가져온 정보 유지");
    const after = librarySchema.parse(
      await (
        await second.request.get(
          `/api/v1/workspaces/${workspace.id}/zotero/libraries/${connector.id}`,
        )
      ).json(),
    );
    expect(after.references[0]?.id).toBe(committed.id);
    expect(after.connector.state).toBe("disconnected");
    expect(after.connector.completedVersion).toBe("13");
    const note = z
      .object({ title: z.string() })
      .passthrough()
      .parse(
        await (
          await second.request.get(
            `/api/v1/workspaces/${workspace.id}/documents/${authored.documentId}`,
          )
        ).json(),
      );
    expect(note.title).toBe("Compare my authored evidence");
    const preserved = await second.request.get(
      `/api/v1/workspaces/${workspace.id}/documents/${authored.documentId}/body`,
    );
    expect(preserved.status()).toBe(200);
    const preservedBody = z.object({ contentJson: z.unknown() }).parse(await preserved.json());
    expect(preservedBody.contentJson).toEqual(authoredBody);
    const requests = z
      .object({ requests: z.array(z.tuple([z.string(), z.string()])) })
      .parse(await fixture.command({ command: "requests" }));
    expect(requests.requests.length).toBeGreaterThan(7);
    for (const [method, url] of requests.requests) {
      expect(method).toBe("GET");
      expect(url).not.toMatch(/\/file|\/children|example\.invalid/);
      expect(url).not.toContain(key);
    }
    const observerPath = info.outputPath("zotero-restricted-new-connection.json");
    await writeFile(observerPath, JSON.stringify(observer));
    await info.attach("zotero-restricted-new-connection.json", {
      path: observerPath,
      contentType: "application/json",
    });
    const servedAssets = await Promise.all(assetReads);
    const assetsPath = info.outputPath("zotero-served-static-assets.json");
    await writeFile(assetsPath, JSON.stringify(servedAssets));
    await info.attach("zotero-served-static-assets.json", {
      path: assetsPath,
      contentType: "application/json",
    });
    expect(servedAssets.length).toBeGreaterThan(0);
    expect(servedAssets.every((asset) => asset.matchesDist)).toBe(true);
    expect(servedAssets.every((asset) => !asset.credentialPresent)).toBe(true);
    for (const reads of [mainReads, newClientReads]) {
      const clientAssets = await Promise.all(reads);
      expect(clientAssets.some((asset) => asset.asset.endsWith(".js"))).toBe(true);
      expect(clientAssets.some((asset) => asset.asset.endsWith(".css"))).toBe(true);
    }
  } finally {
    try {
      try {
        await second?.close();
      } finally {
        await context.close();
      }
    } finally {
      await fixture.stop();
    }
  }
});
