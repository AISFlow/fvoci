import { expect, test } from "bun:test";
import { resolve } from "node:path";

const valid = {
  FVOCI_E2E_SELECTED_BACKEND: "sqlite",
  FVOCI_E2E_SELECTED_FLOW: "on",
  FVOCI_E2E_AUTH_SOURCE: "a".repeat(40),
  FVOCI_E2E_AUTH_TREE: "b".repeat(40),
  FVOCI_E2E_SMTP_CAPTURE: "/owned/private/smtp.jsonl",
  PLAYWRIGHT_BASE_URL: "http://127.0.0.1:12345",
};
const path = resolve(import.meta.dir, "auth-session-playwright.config.ts");
const childEnv = () => ({
  PATH: process.env.PATH,
  TMPDIR: process.env.TMPDIR,
  LANG: process.env.LANG,
});

function load(overrides: Record<string, string | undefined> = {}) {
  const env = { ...childEnv(), ...valid, ...overrides };
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
  const faults = [
    ["FVOCI_E2E_SELECTED_BACKEND", null],
    ["FVOCI_E2E_SELECTED_BACKEND", "turso"],
    ["FVOCI_E2E_SELECTED_FLOW", null],
    ["FVOCI_E2E_SELECTED_FLOW", "OFF"],
    ["FVOCI_E2E_AUTH_SOURCE", "short"],
    ["FVOCI_E2E_AUTH_TREE", null],
    ["FVOCI_E2E_SMTP_CAPTURE", null],
    ["FVOCI_E2E_SMTP_CAPTURE", "relative"],
    ["PLAYWRIGHT_BASE_URL", null],
    ["FVOCI_E2E_PENDING", "1"],
  ];
  // Import the actual config afresh for each input, sharing one process/SDK
  // load instead of charging ten Bun startups to this one five-second test.
  // Valid controls at both ends prove cache-busted imports really resolve.
  const child = Bun.spawnSync(
    [
      process.execPath,
      "--eval",
      `
    const outputs=[];
    const variants=[null,...${JSON.stringify(faults)},null];
    for (const [index,fault] of variants.entries()) {
      for (const name of Object.keys(process.env))
        if(name.startsWith("FVOCI_E2E_")||name==="PLAYWRIGHT_BASE_URL"||name==="CI")
          Reflect.deleteProperty(process.env,name);
      Object.assign(process.env,${JSON.stringify(valid)});
      if(fault) {
        if(fault[1]===null) Reflect.deleteProperty(process.env,fault[0]);
        else process.env[fault[0]]=fault[1];
      }
      try {
        const {default:c}=await import(${JSON.stringify(path)}+"?auth-case="+index);
        outputs.push(c.metadata.authSession===true&&c.metadata.selectedBackend==="sqlite"?0:2);
      } catch { outputs.push(1); }
    }
    console.log(JSON.stringify(outputs));
  `,
    ],
    { env: childEnv(), stdout: "pipe", stderr: "pipe" },
  );
  expect(child.exitCode).toBe(0);
  expect(JSON.parse(child.stdout.toString())).toEqual([0, ...faults.map(() => 1), 0]);
});
