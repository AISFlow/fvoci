/**
 * Project documents on the same /collab room, persist barrier and derived body
 * as wiki documents, under project permission. Runs in the collaboration-flow
 * job after workspace-wiki-collab.spec.ts (same worker server; that spec owns
 * instance setup). Invocation: FVOCI_E2E_PENDING=1 bash scripts/run-web-e2e.sh
 */
import {
  admin,
  closeCollabContext,
  editorShape,
  ensureCollabFixture,
  expect,
  expectConverged,
  expectTokens,
  installCollabPeer,
  login,
  member,
  newCollabContext,
  openEditor,
  persistBody,
  test,
  waitConnected,
  workspaceId,
} from "./collab-helpers";
import type { Page } from "@playwright/test";

/** Own user: workspace-wiki-collab.spec.ts revokes `peer` earlier in the same run. */
const projectPeer = {
  email: "project-collab-peer@example.com",
  password: "projectpeer1",
  givenName: "프로젝트",
  familyName: "동료",
};

type ProjectDoc = {
  workspaceId: string;
  projectId: string;
  documentId: string;
  url: string;
};

async function createPrivateProjectDoc(page: Page, key: string): Promise<ProjectDoc> {
  const wsId = await workspaceId(page, admin.workspaceSlug);
  const projectRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key, name: `${key} 협업`, visibility: "private" },
  });
  expect(projectRes.status()).toBe(201);
  const project = await projectRes.json();
  const docRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/documents`,
    { data: { parentId: project.rootDocumentId, title: "프로젝트 협업 문서" } },
  );
  expect(docRes.status()).toBe(201);
  const doc = await docRes.json();
  return {
    workspaceId: wsId,
    projectId: project.id,
    documentId: doc.id,
    url: `/w/${admin.workspaceSlug}/${doc.displayId}`,
  };
}

async function memberUserId(page: Page, wsId: string, email: string): Promise<string> {
  const res = await page.request.get(`/api/v1/workspaces/${wsId}/members`);
  expect(res.ok()).toBe(true);
  const userId = (await res.json()).items.find(
    (item: { email: string }) => item.email.toLowerCase() === email.toLowerCase(),
  )?.userId;
  expect(userId).toBeTruthy();
  return userId;
}

test("project document edits persist, project the body, and reload", async ({ page }) => {
  await ensureCollabFixture(page);
  await login(page, member.email, member.password);
  const doc = await createPrivateProjectDoc(page, "PCOL");
  const editor = await openEditor(page, doc.url);
  await expect(page.locator(".document-page__breadcrumb")).toContainText("PCOL");
  await editor.click();
  await page.keyboard.type("프로젝트 본문 한글🙂");
  await persistBody(page);

  const bodyRes = await page.request.get(
    `/api/v1/workspaces/${doc.workspaceId}/projects/${doc.projectId}/documents/${doc.documentId}/body`,
  );
  expect(bodyRes.ok()).toBe(true);
  // The derived Tiptap body keeps the text and stores the emoji as an emoji node.
  const derived = JSON.stringify((await bodyRes.json()).contentJson);
  expect(derived).toContain("프로젝트 본문 한글");
  expect(derived).toContain('"type":"emoji"');

  await page.reload();
  await waitConnected(page);
  expect((await editorShape(page)).text).toContain("프로젝트 본문 한글🙂");
});

test("private project document: outsider refused, granted member co-edits", async ({
  browser,
  collabApp,
}) => {
  installCollabPeer(projectPeer);
  const ctxA = await newCollabContext(browser, collabApp.baseUrl);
  const ctxB = await newCollabContext(browser, collabApp.baseUrl);
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  let bodyFailed = true;
  try {
    await login(pageA, member.email, member.password);
    const doc = await createPrivateProjectDoc(pageA, "PPEER");
    await login(pageB, projectPeer.email, projectPeer.password);

    const refused = await pageB.request.get(
      `/api/v1/workspaces/${doc.workspaceId}/projects/${doc.projectId}/documents/${doc.documentId}/body`,
    );
    expect(refused.status()).toBe(404);
    await pageB.goto(doc.url);
    await expect(pageB.locator(".fvoci-editor .ProseMirror")).toHaveCount(0);

    const peerId = await memberUserId(pageA, doc.workspaceId, projectPeer.email);
    const grant = await pageA.request.post(
      `/api/v1/workspaces/${doc.workspaceId}/projects/${doc.projectId}/members`,
      { data: { userId: peerId, role: "member" } },
    );
    expect(grant.status()).toBe(201);

    const editorA = await openEditor(pageA, doc.url);
    const editorB = await openEditor(pageB, doc.url);
    await editorA.click();
    await pageA.keyboard.type("가나다");
    await expectTokens(pageB, ["가나다"]);
    await editorB.click();
    await pageB.keyboard.press("End");
    await pageB.keyboard.type("🙂동료");
    await expectTokens(pageA, ["🙂동료"]);
    await expectConverged(pageA, pageB);
    bodyFailed = false;
  } finally {
    await closeCollabContext(ctxA, bodyFailed);
    await closeCollabContext(ctxB, bodyFailed);
  }
});
