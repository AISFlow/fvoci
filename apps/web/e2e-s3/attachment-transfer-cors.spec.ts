// Presigned upload against storage whose CORS allows another origin only
// (scripts/run-web-e2e-s3.sh --narrow-cors starts MinIO with
// MINIO_API_CORS_ALLOW_ORIGIN): the browser cannot PUT a part, the upload
// fails with an error, and nothing falls back to the API part path.
import { expect, test } from "@playwright/test";
import { watchCspViolations } from "../e2e/helpers";
import { setTransferMode, setUpOwnerTask, storageOrigin } from "./transfer-helpers";

function allowedOrigin(): string {
  const origin = process.env.FVOCI_E2E_S3_CORS_ALLOW_ORIGIN;
  if (!origin)
    throw new Error(
      "FVOCI_E2E_S3_CORS_ALLOW_ORIGIN is required (scripts/run-web-e2e-s3.sh --narrow-cors)",
    );
  return origin;
}

test("storage CORS for another origin fails the presigned upload without an API fallback", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const storage = storageOrigin();
  const other = allowedOrigin();
  const csp = watchCspViolations(page);

  const taskUrl = await setUpOwnerTask(page);
  const appOrigin = new URL(page.url()).origin;
  expect(other).not.toBe(appOrigin);

  // Storage answers the preflight for the allowed origin and not for the app.
  const bucket = process.env.S3_BUCKET ?? "";
  const preflight = (origin: string) =>
    page.request.fetch(`${storage}/${bucket}/cors-probe?partNumber=1&uploadId=x`, {
      method: "OPTIONS",
      headers: { Origin: origin, "Access-Control-Request-Method": "PUT" },
    });
  expect((await preflight(other)).headers()["access-control-allow-origin"]).toBe(other);
  expect((await preflight(appOrigin)).headers()["access-control-allow-origin"]).toBeUndefined();

  await setTransferMode(page, "presigned", "직접 전송 (S3 서명 URL)");
  await page.goto(taskUrl);

  const storagePuts: string[] = [];
  const failedStoragePuts: string[] = [];
  const apiCalls: string[] = [];
  page.on("request", (req) => {
    const url = req.url();
    if (req.method() === "PUT" && url.startsWith(`${storage}/`)) storagePuts.push(url);
    if (
      url.startsWith(`${appOrigin}/api/`) &&
      /\/(parts\/|complete$|upload$)/.test(new URL(url).pathname)
    ) {
      apiCalls.push(
        `${req.method()} ${new URL(url).pathname.replace(/.*\/attachments\/[^/]+\//, "")}`,
      );
    }
  });
  page.on("requestfailed", (req) => {
    if (req.method() === "PUT" && req.url().startsWith(`${storage}/`))
      failedStoragePuts.push(req.url());
  });
  const created: Record<string, unknown>[] = [];
  const resumed: Record<string, unknown>[] = [];
  let resumeUrl = "";
  page.on("response", async (res) => {
    const method = res.request().method();
    if (res.url().endsWith("/uploads") && method === "POST" && res.status() === 201) {
      created.push((await res.json()) as Record<string, unknown>);
    }
    if (res.url().endsWith("/upload") && method === "GET" && res.status() === 200) {
      resumeUrl = res.url();
      resumed.push((await res.json()) as Record<string, unknown>);
    }
  });

  const panel = page.getByRole("region", { name: "첨부" });
  await panel
    .getByLabel("파일 첨부")
    .setInputFiles([
      {
        name: "blocked.txt",
        mimeType: "text/plain",
        buffer: Buffer.from("never stored\n", "utf8"),
      },
    ]);
  // The pipeline's error for a transfer that never reached storage.
  await expect(panel.getByRole("alert")).toHaveText("연결을 확인하고 다시 시도해 주세요.", {
    timeout: 60_000,
  });
  await expect(panel.getByRole("link", { name: "blocked.txt" })).toHaveCount(0);

  // Presigned from create through its one resume; every attempt went to
  // storage and failed there (three tries per round, two rounds), and no part
  // PUT or complete reached the API.
  expect(created.map((c) => c.transfer)).toEqual(["presigned"]);
  expect(resumed.map((r) => r.transfer)).toEqual(["presigned"]);
  expect(storagePuts).toHaveLength(6);
  expect(failedStoragePuts).toHaveLength(6);
  expect(apiCalls).toEqual(["GET upload"]);

  // The session stays open in presigned mode: resuming it now still hands out
  // storage URLs, never API part paths.
  const again = await page.request.get(resumeUrl);
  expect(again.status()).toBe(200);
  const session = (await again.json()) as { transfer: string; parts: { url: string }[] };
  expect(session.transfer).toBe("presigned");
  expect(session.parts.length).toBe(1);
  for (const part of session.parts) expect(part.url.startsWith(`${storage}/`)).toBe(true);

  expect(csp).toEqual([]);
});
