// Shared steps of the Vue wiki editor specs (workspace-wiki-vue-controls*.spec.ts):
// the instance and a member, wiki documents through the API, the page with
// its connected collab room, and the editor's top-level blocks.
import {
  type APIRequestContext,
  type Browser,
  type BrowserContext,
  expect,
  type Locator,
  type Page,
} from "@playwright/test";
import { readJson, flowSchemas, createE2eUser, login } from "./helpers";

export const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "vctl",
  workspaceName: "Vue Editor Controls",
};

export const member = {
  email: "vue-controls-member@example.com",
  password: "memberpass1",
  givenName: "편집",
  familyName: "동료",
};

// Icon sets must be bundled; these are the Iconify API hosts a runtime fetch would hit.
const ICON_API_HOSTS = ["api.iconify.design", "api.simplesvg.com", "api.unisvg.com"];

export function watchIconRequests(page: Page): string[] {
  const hits: string[] = [];
  page.on("request", (request) => {
    const host = new URL(request.url()).hostname;
    if (ICON_API_HOSTS.includes(host)) {
      hits.push(request.url());
    }
  });
  return hits;
}

/** First run of the group: the setup form creates the admin and the workspace
 * (true); later runs sign in (false). */
export async function ensureSetup(page: Page, setupAdmin = admin): Promise<boolean> {
  let created = false;
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if ((await page.getByRole("button", { name: "시작하기" }).count()) > 0) {
    await page.getByLabel("성").fill(setupAdmin.familyName);
    await page.getByLabel("이름", { exact: true }).fill(setupAdmin.givenName);
    await page.getByLabel("이메일").fill(setupAdmin.email);
    await page.getByLabel("비밀번호").fill(setupAdmin.password);
    await page.getByLabel("워크스페이스 이름").fill(setupAdmin.workspaceName);
    await page.getByLabel("주소(영문)").fill(setupAdmin.workspaceSlug);
    await page.getByRole("button", { name: "시작하기" }).click();
    created = true;
  } else {
    if (
      page.url().includes("/login") ||
      (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
    ) {
      await login(page, setupAdmin.email, setupAdmin.password);
    }
  }
  // Setup starts at '/', then may cross Vue /login before returning home.
  // Do not close the setup context until authenticated home has rendered.
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
  return created;
}

/** The instance, its admin and a workspace member, made on the group's
 * fresh database (a repeated run finds them). */
export async function setupInstance(
  browser: Browser,
  baseURL: string | undefined,
  isolatedAdmin?: typeof admin,
): Promise<void> {
  const context = await browser.newContext({ baseURL });
  const setupAdmin = isolatedAdmin ?? admin;
  try {
    if (isolatedAdmin !== undefined) {
      // Repeated entity runs seed their own owner without charging another
      // login to the original setup account or the shared client address.
      const status = await context.request.get("/api/v1/setup");
      expect(status.ok(), await status.text()).toBe(true);
      if (!(await readJson(status, flowSchemas.setup)).needed) {
        createE2eUser(setupAdmin.email, setupAdmin.password, setupAdmin.givenName, {
          familyName: setupAdmin.familyName,
          workspaceSlug: setupAdmin.workspaceSlug,
          membershipRole: "owner",
        });
        return;
      }
    }
    if (!(await ensureSetup(await context.newPage(), setupAdmin))) {
      return;
    }
    createE2eUser(member.email, member.password, member.givenName, {
      familyName: member.familyName,
      workspaceSlug: setupAdmin.workspaceSlug,
      membershipRole: "member",
    });
  } finally {
    await context.close();
  }
}

export async function newSignedInPage(
  browser: Browser,
  baseURL: string | undefined,
  who: {
    email: string;
    password: string;
  },
  options: {
    permissions?: string[];
  } = {},
): Promise<{
  context: BrowserContext;
  page: Page;
}> {
  const context = await browser.newContext({ baseURL, permissions: options.permissions ?? [] });
  const page = await context.newPage();
  await login(page, who.email, who.password);
  return { context, page };
}

export async function workspaceId(request: APIRequestContext): Promise<string> {
  const res = await request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const workspace = (await readJson(res, flowSchemas.workspaces)).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  if (workspace === undefined) throw new Error("Missing fixture value: workspace");
  expect(workspace).toBeTruthy();
  return workspace.id;
}
export type WikiDoc = {
  id: string;
  number: number;
  path: string;
};
export async function createDoc(
  request: APIRequestContext,
  wsId: string,
  title: string,
  body?:
    | {
        markdown: string;
      }
    | {
        json: unknown;
      },
): Promise<WikiDoc> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { commandId: crypto.randomUUID(), parentId: null, title },
  });
  expect(res.status(), await res.text()).toBe(201);
  const doc = (await readJson(res, flowSchemas.document)) as {
    id: string;
    number: number;
  };
  if (body !== undefined) {
    const put = await request.put(`/api/v1/workspaces/${wsId}/documents/${doc.id}/body`, {
      data: "markdown" in body ? { contentMd: body.markdown } : { contentJson: body.json },
    });
    expect(put.ok(), await put.text()).toBe(true);
  }
  return { ...doc, path: `/w/${admin.workspaceSlug}/WIKI-${String(doc.number)}` };
}

/** The saved body (REST), as Tiptap JSON. */
export async function savedBody(
  request: APIRequestContext,
  wsId: string,
  docId: string,
): Promise<TiptapNode> {
  const res = await request.get(`/api/v1/workspaces/${wsId}/documents/${docId}/body`);
  expect(res.ok()).toBe(true);
  return (await readJson(res, flowSchemas.body)).contentJson as TiptapNode;
}

export type TiptapNode = {
  type: string;
  attrs?: Record<string, unknown>;
  content?: TiptapNode[];
  text?: string;
  marks?: {
    type: string;
    attrs?: Record<string, unknown>;
  }[];
};

export function editorOf(page: Page): Locator {
  return page.locator(".fvoci-editor .ProseMirror");
}

export async function openDoc(page: Page, path: string): Promise<Locator> {
  const navigation = await page.goto(path);
  expect(navigation?.status()).toBe(200);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15000 });
  const editor = editorOf(page);
  await expect(editor).toBeVisible();
  return editor;
}

/** Top-level block texts without peer caret labels (they are editor decorations). */
export async function blockTexts(page: Page): Promise<string[]> {
  return editorOf(page).evaluate((root) =>
    [...root.children].map((block) => {
      const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT, {
        acceptNode: (node) =>
          node.parentElement?.closest(".collaboration-carets__caret, .collaboration-carets__label")
            ? NodeFilter.FILTER_REJECT
            : NodeFilter.FILTER_ACCEPT,
      });
      let text = "";
      while (walker.nextNode()) {
        const content = walker.currentNode.textContent;
        if (content === null) throw new Error("Text walker must visit text nodes");
        text += content;
      }
      return text;
    }),
  );
}

export async function expectBlocks(page: Page, expected: string[]): Promise<void> {
  await expect.poll(() => blockTexts(page), { timeout: 15000 }).toEqual(expected);
}

/** Top-level block `index` of the editor. */
export function blockAt(page: Page, index: number): Locator {
  return editorOf(page).locator(":scope > *").nth(index);
}

/** Puts the caret at the end of top-level block `index` with a real click and End key. */
export async function caretAtEndOf(page: Page, index: number): Promise<void> {
  await blockAt(page, index).click();
  await page.keyboard.press("End");
}

/** The Save button: flush, then the persist ACK. */
export async function save(page: Page): Promise<void> {
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15000 });
}

/** The element that has focus: its tag, role and accessible label, or "body". */
export async function focused(page: Page): Promise<string> {
  return page.evaluate(() => {
    const active = document.activeElement;
    if (!active || active === document.body) {
      return "body";
    }
    if (active.closest(".ProseMirror")) {
      return "editor";
    }
    const label = active.getAttribute("aria-label") ?? active.textContent.trim();
    return `${active.getAttribute("role") ?? active.tagName.toLowerCase()}:${label}`;
  });
}
