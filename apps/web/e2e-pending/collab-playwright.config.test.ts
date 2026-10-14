import { expect, test } from "bun:test";
import { resolve, join } from "node:path";
import { mkdtempSync, rmSync } from "node:fs";
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
       const names=["workspace-wiki-selected-backend.spec.ts","workspace-off-selected-backend.spec.ts","v060-sqlite-revision-restore.spec.ts","v060-equal-work.spec.ts","wiki-collab.spec.ts"];
       const ignore=[c.testIgnore].flat();
       console.log(JSON.stringify({names:names.filter(n=>c.testMatch.test(n)&&!ignore.some(p=>p.test(n))),metadata:c.metadata,workers:c.workers,retries:c.retries}));`,
    ],
    { env: { ...clean, ...env }, stdout: "pipe", stderr: "pipe" },
  );
  return {
    code: result.exitCode,
    output: result.stdout.toString(),
    error: result.stderr.toString(),
  };
}

test("pending discovery keeps pending specs; both allocated normal flows and the dedicated restore spec are excluded", () => {
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
  // These cases vary only the binding file. Import the same real fixture once,
  // then require it to reread and validate every changed file in the child.
  try {
    const result = Bun.spawnSync(
      [
        process.execPath,
        "--eval",
        `import { strict as assert } from "node:assert";
         import { chmodSync, writeFileSync } from "node:fs";
         const {selectedSetupNeeded}=await import(${JSON.stringify(fixturePath)});
         const path=${JSON.stringify(path)};
         const input=${JSON.stringify(input)};
         function observe(binding) {
           writeFileSync(path, JSON.stringify(binding));
           chmodSync(path, 0o600);
           return selectedSetupNeeded();
         }
         const initialized=observe(input);
         assert.equal(initialized, false);
         const empty=observe({...input, setupNeeded:true, commit:"not-attempted"});
         assert.equal(empty, true);
         const changed=[
           {namespace:"acme"},
           {source:"c".repeat(40)},
           {lifecycleDrain:"failed"},
           {leases:1},
           {schemaCurrent:false},
           {setupNeeded:"false"},
         ];
         for (const mutation of changed) {
           assert.throws(() => observe({...input, ...mutation}));
         }
         console.log(JSON.stringify({initialized, empty, refused:changed.length}));`,
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
    expect(result.exitCode).toBe(0);
    expect(JSON.parse(result.stdout.toString())).toEqual({
      initialized: false,
      empty: true,
      refused: 6,
    });
  } finally {
    rmSync(directory, { recursive: true });
  }
});
