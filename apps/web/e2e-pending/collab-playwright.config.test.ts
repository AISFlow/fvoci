import { expect, test } from "bun:test";
import { resolve, join } from "node:path";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

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

for (const backend of ["postgres", "sqlite", "libsql-remote"]) {
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

test("remote setup follows the captured target state and rejects changed allocation or native drain", () => {
  const directory = mkdtempSync(join(tmpdir(), "fvoci-turso-binding-"));
  const path = join(directory, "binding.json");
  const namespace = "tui-0123456789abcdef0123";
  const input = {
    schema: 1,
    backend: "libsql-remote",
    setupNeeded: false,
    namespace,
    ownerEmail: `${namespace}-owner@example.invalid`,
    memberEmail: `${namespace}-member@example.invalid`,
    workspaceSlug: namespace,
    source: "a".repeat(40),
    tree: "b".repeat(40),
    schemaCurrent: true,
    commit: "confirmed",
    lifecycleDrain: "confirmed",
    leases: 0,
  };
  const fixturePath = resolve(import.meta.dir, "selected-backend-fixture.ts");
  function run(binding: Record<string, unknown>) {
    writeFileSync(path, JSON.stringify(binding));
    chmodSync(path, 0o600);
    return Bun.spawnSync(
      [
        process.execPath,
        "--eval",
        `const {selectedSetupNeeded}=await import(${JSON.stringify(fixturePath)}); console.log(selectedSetupNeeded());`,
      ],
      {
        env: {
          PATH: process.env.PATH,
          FVOCI_E2E_SELECTED_BACKEND: "libsql-remote",
          FVOCI_E2E_TURSO_NAMESPACE: namespace,
          FVOCI_E2E_TURSO_ACTOR_BINDING: path,
          FVOCI_E2E_TURSO_SOURCE: input.source,
          FVOCI_E2E_TURSO_TREE: input.tree,
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
  }
  try {
    const initialized = run(input);
    expect(initialized.exitCode).toBe(0);
    expect(initialized.stdout.toString().trim()).toBe("false");
    const empty = run({ ...input, setupNeeded: true, commit: "not-attempted" });
    expect(empty.exitCode).toBe(0);
    expect(empty.stdout.toString().trim()).toBe("true");
    for (const changed of [
      { namespace: "acme" },
      { source: "c".repeat(40) },
      { lifecycleDrain: "failed" },
      { leases: 1 },
      { schemaCurrent: false },
      { setupNeeded: "false" },
    ]) {
      expect(run({ ...input, ...changed }).exitCode).not.toBe(0);
    }
  } finally {
    rmSync(directory, { recursive: true });
  }
});
