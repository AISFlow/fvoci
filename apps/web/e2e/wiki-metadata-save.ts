import { expect, type Page, type Request, type Response, type Route } from "@playwright/test";
import {
  createHoldGate,
  documentResourcePath,
  isDocumentResourceUrl,
  isSuccessfulMatchingDocumentPatch,
  parseJsonBody,
  patchBodyMatches,
  type DocumentIds,
  type DocumentPatchBody,
} from "./document-save-barrier.ts";

export type { DocumentIds, DocumentPatchBody };
export { createHoldGate, documentResourcePath };

export async function wikiDocumentIdsFromPage(page: Page, slug: string): Promise<DocumentIds> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = (await workspacesRes.json()) as {
    items: { id: string; slug: string }[];
  };
  const workspace = workspacesBody.items.find((item) => item.slug === slug);
  expect(workspace).toBeTruthy();

  const match = /\/WIKI-(\d+)$/.exec(new URL(page.url()).pathname);
  expect(match).toBeTruthy();
  const number = Number(match![1]);

  const treeRes = await page.request.get(`/api/v1/workspaces/${workspace!.id}/tree`);
  expect(treeRes.ok()).toBe(true);
  const treeBody = (await treeRes.json()) as {
    items: { id: string; number: number }[];
  };
  const document = treeBody.items.find((item) => item.number === number);
  expect(document).toBeTruthy();
  return { workspaceId: workspace!.id, documentId: document!.id };
}

export async function wikiDocumentIdsByTitle(
  page: Page,
  slug: string,
  title: string,
): Promise<DocumentIds> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = (await workspacesRes.json()) as {
    items: { id: string; slug: string }[];
  };
  const workspace = workspacesBody.items.find((item) => item.slug === slug);
  expect(workspace).toBeTruthy();

  const treeRes = await page.request.get(`/api/v1/workspaces/${workspace!.id}/tree`);
  expect(treeRes.ok()).toBe(true);
  const treeBody = (await treeRes.json()) as {
    items: { id: string; title: string }[];
  };
  const document = treeBody.items.find((item) => item.title === title);
  expect(document).toBeTruthy();
  return { workspaceId: workspace!.id, documentId: document!.id };
}

export async function fetchDocumentMeta(
  page: Page,
  ids: DocumentIds,
): Promise<{ title: string; icon: string | null; status: string }> {
  const res = await page.request.get(documentResourcePath(ids));
  expect(res.ok()).toBe(true);
  return res.json() as Promise<{ title: string; icon: string | null; status: string }>;
}

function requestBodyOf(request: Request): unknown {
  return parseJsonBody(request.postData());
}

export function waitForSuccessfulDocumentPatch(
  page: Page,
  ids: DocumentIds,
  expected: DocumentPatchBody,
): Promise<Response> {
  return page.waitForResponse(async (response) => {
    if (response.request().method() !== "PATCH" || !response.ok()) {
      return false;
    }
    if (!isDocumentResourceUrl(response.url(), ids)) {
      return false;
    }
    let responseBody: unknown;
    try {
      responseBody = await response.json();
    } catch {
      return false;
    }
    return isSuccessfulMatchingDocumentPatch({
      method: response.request().method(),
      url: response.url(),
      ok: response.ok(),
      workspaceId: ids.workspaceId,
      documentId: ids.documentId,
      requestBody: requestBodyOf(response.request()),
      responseBody,
      expected,
    });
  });
}

export async function holdMatchingDocumentPatch(
  page: Page,
  ids: DocumentIds,
  expected: DocumentPatchBody,
  held: Promise<void>,
): Promise<{ unroute: () => Promise<void> }> {
  const matchUrl = (url: URL) => isDocumentResourceUrl(url.href, ids);
  const handler = async (route: Route) => {
    const request = route.request();
    if (
      request.method() === "PATCH" &&
      patchBodyMatches(requestBodyOf(request), expected)
    ) {
      await held;
    }
    await route.continue();
  };
  await page.route(matchUrl, handler);
  return {
    unroute: () => page.unroute(matchUrl, handler),
  };
}
