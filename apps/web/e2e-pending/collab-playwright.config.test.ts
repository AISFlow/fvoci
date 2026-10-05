import { expect, test } from "bun:test";
import { resolve } from "node:path";

const configPath = resolve(import.meta.dir, "collab-playwright.config.ts");

function selected(env: Record<string, string> = {}) {
  const selectionNames = new Set([
    "CI",
    "FVOCI_E2E_SELECTED_BACKEND",
    "FVOCI_E2E_SELECTED_FLOW",
    "FVOCI_E2E_PENDING",
  ]);
  const clean = Object.fromEntries(
    Object.entries(process.env).filter(([name]) => !selectionNames.has(name)),
  );
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--eval",
      `const {default:c}=await import(${JSON.stringify(configPath)});
       const names=["workspace-wiki-selected-backend.spec.ts","workspace-off-selected-backend.spec.ts","wiki-collab.spec.ts"];
       console.log(JSON.stringify({names:names.filter(n=>c.testMatch.test(n)&&!(c.testIgnore instanceof RegExp&&c.testIgnore.test(n))),metadata:c.metadata,workers:c.workers,retries:c.retries}));`,
    ],
    { env: { ...clean, ...env }, stdout: "pipe", stderr: "pipe" },
  );
  return {
    code: result.exitCode,
    output: result.stdout.toString(),
    error: result.stderr.toString(),
  };
}

test("pending discovery keeps pending specs; both allocated normal flows belong to the mandatory companion", () => {
  const result = selected({ FVOCI_E2E_PENDING: "1" });
  expect(result.code).toBe(0);
  expect(JSON.parse(result.output)).toEqual({
    names: ["wiki-collab.spec.ts"],
    metadata: { selectedBackend: null, selectedFlow: null },
    workers: 1,
    retries: 0,
  });
});

for (const backend of ["postgres", "sqlite"]) {
  test(`${backend} ON default preserves the normal tracer selection`, () => {
    const result = selected({ FVOCI_E2E_SELECTED_BACKEND: backend });
    expect(result.code).toBe(0);
    expect(JSON.parse(result.output)).toEqual({
      names: ["workspace-wiki-selected-backend.spec.ts"],
      metadata: { selectedBackend: backend, selectedFlow: "on" },
      workers: 1,
      retries: 0,
    });
  });
  test(`${backend} OFF selects only the unchanged seven-case spec`, () => {
    const result = selected({
      FVOCI_E2E_SELECTED_BACKEND: backend,
      FVOCI_E2E_SELECTED_FLOW: "off",
    });
    expect(result.code).toBe(0);
    expect(JSON.parse(result.output)).toEqual({
      names: ["workspace-off-selected-backend.spec.ts"],
      metadata: { selectedBackend: backend, selectedFlow: "off" },
      workers: 1,
      retries: 0,
    });
  });
}

test("missing backend, unsupported flow/backend, and pending/normal lifecycle collisions fail before discovery", () => {
  for (const env of [
    { FVOCI_E2E_SELECTED_FLOW: "off" },
    { FVOCI_E2E_SELECTED_FLOW: "on" },
    { FVOCI_E2E_SELECTED_BACKEND: "turso", FVOCI_E2E_SELECTED_FLOW: "off" },
    { FVOCI_E2E_SELECTED_BACKEND: "sqlite", FVOCI_E2E_SELECTED_FLOW: "OFF" },
    { FVOCI_E2E_SELECTED_BACKEND: "postgres", FVOCI_E2E_SELECTED_FLOW: "other" },
    {
      FVOCI_E2E_SELECTED_BACKEND: "sqlite",
      FVOCI_E2E_SELECTED_FLOW: "off",
      FVOCI_E2E_PENDING: "1",
    },
    { FVOCI_E2E_SELECTED_BACKEND: "postgres", FVOCI_E2E_PENDING: "1" },
  ]) {
    expect(selected(env).code).not.toBe(0);
  }
});
