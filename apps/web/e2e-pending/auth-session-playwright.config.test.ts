import { expect, test } from "bun:test";
import { resolve } from "node:path";

function load(overrides: Record<string, string | undefined> = {}) {
  const env = { ...process.env };
  for (const key of Object.keys(env))
    if (key.startsWith("FVOCI_E2E_") || key === "PLAYWRIGHT_BASE_URL" || key === "CI")
      Reflect.deleteProperty(env, key);
  Object.assign(env, {
    FVOCI_E2E_SELECTED_BACKEND: "sqlite",
    FVOCI_E2E_SELECTED_FLOW: "on",
    FVOCI_E2E_AUTH_SOURCE: "a".repeat(40),
    FVOCI_E2E_AUTH_TREE: "b".repeat(40),
    FVOCI_E2E_SMTP_CAPTURE: "/owned/private/smtp.jsonl",
    PLAYWRIGHT_BASE_URL: "http://127.0.0.1:12345",
    ...overrides,
  });
  const path = resolve(import.meta.dir, "auth-session-playwright.config.ts");
  const child = Bun.spawnSync(
    [
      process.execPath,
      "--eval",
      `
    const {default:c}=await import(${JSON.stringify(path)});
    console.log(JSON.stringify({match:c.testMatch,workers:c.workers,retries:c.retries,
      metadata:c.metadata,trace:c.use.trace,screenshot:c.use.screenshot}));
  `,
    ],
    { env, stdout: "pipe", stderr: "pipe" },
  );
  return { code: child.exitCode, output: child.stdout.toString() };
}

for (const backend of ["postgres", "sqlite", "libsql-remote"]) {
  for (const flow of ["on", "off"]) {
    test(`${backend} ${flow}: explicit auth discovery preserves original spec discovery`, () => {
      const result = load({ FVOCI_E2E_SELECTED_BACKEND: backend, FVOCI_E2E_SELECTED_FLOW: flow });
      expect(result.code).toBe(0);
      expect(JSON.parse(result.output)).toEqual({
        match: "**/auth-session-selected-backend.acceptance.ts",
        workers: 1,
        retries: 0,
        metadata: {
          authSession: true,
          selectedBackend: backend,
          selectedFlow: flow,
          source: "a".repeat(40),
          tree: "b".repeat(40),
        },
        trace: "off",
        screenshot: "off",
      });
      expect("auth-session-selected-backend.acceptance.ts".endsWith(".spec.ts")).toBe(false);
    });
  }
}

test("missing binding/capture/backend/flow and pending lifecycle fail before discovery", () => {
  for (const overrides of [
    { FVOCI_E2E_SELECTED_BACKEND: undefined },
    { FVOCI_E2E_SELECTED_BACKEND: "turso" },
    { FVOCI_E2E_SELECTED_FLOW: undefined },
    { FVOCI_E2E_SELECTED_FLOW: "OFF" },
    { FVOCI_E2E_AUTH_SOURCE: "short" },
    { FVOCI_E2E_AUTH_TREE: undefined },
    { FVOCI_E2E_SMTP_CAPTURE: undefined },
    { FVOCI_E2E_SMTP_CAPTURE: "relative" },
    { PLAYWRIGHT_BASE_URL: undefined },
    { FVOCI_E2E_PENDING: "1" },
  ])
    expect(load(overrides).code).not.toBe(0);
});
