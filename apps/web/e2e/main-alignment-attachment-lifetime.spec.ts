// Transport fault injection runs over the production Vue/Rust/DB stack.
// The local group uses API transfer; presigned storage is verified separately.
import { expect, test, type Page } from "@playwright/test";
import { z } from "zod";
import { flowSchemas, login, readJson } from "./helpers";

const owner = {
  email: "attachment-lifetime@example.com",
  password: "supersecret1",
  slug: "lifetime",
};

async function pasteFile(page: Page, name: string): Promise<void> {
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await editor.click();
  await editor.evaluate((element, name) => {
    const clipboardData = new DataTransfer();
    clipboardData.items.add(new File([`original bytes: ${name}`], name, { type: "text/plain" }));
    element.dispatchEvent(new ClipboardEvent("paste", { clipboardData, bubbles: true }));
  }, name);
}

test("queued attachments survive complete loss/retry and cannot insert after cancel or document switch", async ({
  page,
  browser,
}, testInfo) => {
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("첨부");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Attachment lifetime");
  await page.getByLabel("주소(영문)").fill(owner.slug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  const workspaces = await readJson(
    await page.request.get("/api/v1/me/workspaces"),
    flowSchemas.workspaces,
  );
  const workspace = workspaces.items.find((item) => item.slug === owner.slug);
  if (!workspace) throw new Error("setup workspace missing");
  const ws = `/api/v1/workspaces/${workspace.id}`;
  const createDoc = async (title: string) => {
    const response = await page.request.post(`${ws}/documents`, {
      data: { parentId: null, title },
    });
    expect(response.status()).toBe(201);
    return readJson(response, flowSchemas.document);
  };
  const source = await createDoc("Upload source");
  const destination = await createDoc("Upload destination");
  const moved = await page.request.post(`${ws}/documents/${source.id}/move`, {
    data: { newParentId: destination.id },
  });
  expect(moved.ok()).toBe(true);
  const sourcePath = `/w/${owner.slug}/WIKI-${String(source.number)}`;
  const destinationPath = `/w/${owner.slug}/WIKI-${String(destination.number)}`;
  await page.goto(sourcePath);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
  const queue = page.locator("[data-fvoci-uploads]");
  const editor = page.locator(".fvoci-editor .ProseMirror");
  const completePattern = "**/attachments/*/complete";
  const partPattern = "**/attachments/*/parts/*";

  // The server commits complete, but the browser loses its response. Metadata
  // reconciliation inserts that same durable ID, without creating another file.
  let lostId = "";
  let completeCalls = 0;
  await page.route(completePattern, async (route) => {
    completeCalls += 1;
    lostId = new URL(route.request().url()).pathname.split("/").at(-2) ?? "";
    const response = await route.fetch();
    expect(response.ok()).toBe(true);
    await route.abort("connectionreset");
  });
  await pasteFile(page, "response-lost.txt");
  await expect(editor.getByRole("link", { name: "response-lost.txt" })).toBeVisible();
  await expect(queue).toHaveCount(0);
  expect(completeCalls).toBe(1);
  expect(lostId).toMatch(/^[0-9a-f-]{36}$/);
  await expect(editor.getByRole("link", { name: "response-lost.txt" })).toHaveAttribute(
    "href",
    `${ws}/attachments/${lostId}/download`,
  );
  await page.unroute(completePattern);

  // A gateway refusal before complete reaches Rust retries the same ID. The
  // second request really completes; a repeated server call is idempotent.
  let retryCalls = 0;
  let retryId = "";
  const retryIds: string[] = [];
  const retryRequest: { body: string | null } = { body: null };
  await page.route(completePattern, async (route) => {
    retryCalls += 1;
    retryId = new URL(route.request().url()).pathname.split("/").at(-2) ?? "";
    retryIds.push(retryId);
    retryRequest.body = route.request().postData();
    if (retryCalls === 1)
      await route.fulfill({ status: 503, contentType: "application/json", body: "{}" });
    else await route.continue();
  });
  await pasteFile(page, "complete-retry.txt");
  await expect(editor.getByRole("link", { name: "complete-retry.txt" })).toBeVisible();
  await expect(queue).toHaveCount(0);
  expect(retryCalls).toBe(2);
  expect(retryIds).toEqual([retryId, retryId]);
  await page.unroute(completePattern);
  if (!retryRequest.body) throw new Error("complete request body missing");
  const repeated = await page.request.post(`${ws}/attachments/${retryId}/complete`, {
    headers: { "content-type": "application/json" },
    data: retryRequest.body,
  });
  expect(repeated.ok()).toBe(true);
  expect((await readJson(repeated, z.object({ id: z.string() }).passthrough())).id).toBe(retryId);

  // A permanent part failure leaves an actionable local retry. Retrying gets
  // a fresh session and clears the temporary queue after the real transfer.
  await page.route(partPattern, (route) => route.fulfill({ status: 413 }));
  const firstSession = page.waitForResponse((response) =>
    response.url().endsWith(`/documents/${source.id}/uploads`),
  );
  await pasteFile(page, "part-retry.txt");
  await expect(queue.getByRole("alert")).toBeVisible();
  const failedUpload = await readJson(await firstSession, flowSchemas.upload);
  await page.unroute(partPattern);
  const retriedSession = page.waitForResponse((response) =>
    response.url().endsWith(`/documents/${source.id}/uploads`),
  );
  await queue.getByRole("button", { name: "재시도" }).click();
  const retriedUpload = await readJson(await retriedSession, flowSchemas.upload);
  expect(retriedUpload.attachmentId).not.toBe(failedUpload.attachmentId);
  await expect(editor.getByRole("link", { name: "part-retry.txt" })).toBeVisible();
  await expect(queue).toHaveCount(0);

  // Cancel while the part is queued at the transport boundary. It must never
  // complete, insert a node, or keep its temporary queue item alive.
  const partEntered = Promise.withResolvers<undefined>();
  const releasePart = Promise.withResolvers<undefined>();
  const partFinished = Promise.withResolvers<undefined>();
  let canceledCompletes = 0;
  await page.route(completePattern, async (route) => {
    canceledCompletes += 1;
    await route.continue();
  });
  await page.route(partPattern, async (route) => {
    partEntered.resolve(undefined);
    await releasePart.promise;
    await route.abort();
    partFinished.resolve(undefined);
  });
  await pasteFile(page, "canceled.txt");
  await partEntered.promise;
  await queue.getByRole("button", { name: "취소" }).click();
  await expect(queue).toHaveCount(0);
  releasePart.resolve(undefined);
  await partFinished.promise;
  await page.unroute(partPattern);
  await page.unroute(completePattern);
  expect(canceledCompletes).toBe(0);
  await expect(editor.getByRole("link", { name: "canceled.txt" })).toHaveCount(0);

  // Complete is durable before navigation but its response remains queued.
  // Navigate through the app, then release it in the same JS realm: an old
  // promise cannot put its result into the destination editor.
  const completeEntered = Promise.withResolvers<undefined>();
  const releaseComplete = Promise.withResolvers<undefined>();
  const completeFinished = Promise.withResolvers<undefined>();
  let lateId = "";
  await page.route(completePattern, async (route) => {
    lateId = new URL(route.request().url()).pathname.split("/").at(-2) ?? "";
    const response = await route.fetch();
    expect(response.ok()).toBe(true);
    completeEntered.resolve(undefined);
    await releaseComplete.promise;
    await route.fulfill({ response });
    completeFinished.resolve(undefined);
  });
  await pasteFile(page, "late-complete.txt");
  await completeEntered.promise;
  await page.getByRole("link", { name: "Upload destination", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`${destinationPath}$`));
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
  releaseComplete.resolve(undefined);
  await completeFinished.promise;
  await page.unroute(completePattern);
  await expect(queue).toHaveCount(0);
  await expect(editor.locator(".afn-attachment")).toHaveCount(0);
  const destinationBody = await readJson(
    await page.request.get(`${ws}/documents/${destination.id}/body`),
    flowSchemas.body,
  );
  expect(JSON.stringify(destinationBody.contentJson)).not.toContain(lateId);
  const lateDownload = await page.request.get(`${ws}/attachments/${lateId}/download`);
  expect(lateDownload.ok()).toBe(true);
  expect(await lateDownload.text()).toBe("original bytes: late-complete.txt");

  // A fresh authenticated browser client reopens the durable source content.
  const fresh = await browser.newContext();
  try {
    const reopened = await fresh.newPage();
    await login(reopened, owner.email, owner.password);
    await reopened.goto(sourcePath);
    await expect(reopened.locator('[data-collab-status="connected"]')).toBeVisible();
    const stored = reopened.locator(".fvoci-editor .ProseMirror");
    for (const name of ["response-lost.txt", "complete-retry.txt", "part-retry.txt"]) {
      const link = stored.getByRole("link", { name });
      await expect(link).toBeVisible();
      const href = await link.getAttribute("href");
      if (!href) throw new Error("durable attachment link missing");
      const bytes = await reopened.request.get(href);
      expect(bytes.ok()).toBe(true);
      expect(await bytes.text()).toBe(`original bytes: ${name}`);
    }
    await expect(stored.getByRole("link", { name: "late-complete.txt" })).toHaveCount(0);
    await expect(stored.getByRole("link", { name: "canceled.txt" })).toHaveCount(0);
    await expect(reopened.locator("[data-fvoci-uploads]")).toHaveCount(0);
  } finally {
    await fresh.close();
  }
  await testInfo.attach("attachment-lifetime", {
    body: JSON.stringify({
      source,
      destination,
      lostId,
      retryId,
      lateId,
      completeCalls,
      retryCalls,
      canceledCompletes,
    }),
    contentType: "application/json",
  });
});
