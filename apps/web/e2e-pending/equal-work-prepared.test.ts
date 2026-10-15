import { expect, test } from "bun:test";
import { strict as assert } from "node:assert";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  workload,
  workloadHash,
} from "../../../tools/selected-backend-ci/drivers/equal-work-policy";
import { equalWorkPreparedMode, parsePreparedEqualWork } from "./equal-work-prepared";

const namespace = "tui-" + "a".repeat(20);
const id = (value: number) => value.toString(16).padStart(8, "0") + "-0000-4000-8000-000000000000";
const expected = {
  backend: "libsql-remote",
  flow: "on",
  namespace,
  source: "a".repeat(40),
  tree: "b".repeat(40),
  targetSha256: "c".repeat(64),
  ownerEmail: namespace + "-owner@example.invalid",
  memberEmail: namespace + "-member@example.invalid",
  workspaceSlug: namespace,
};
function binding() {
  return {
    schema: 1,
    backend: "libsql-remote",
    setupNeeded: false,
    namespace,
    ownerEmail: expected.ownerEmail,
    memberEmail: expected.memberEmail,
    workspaceSlug: namespace,
    source: expected.source,
    tree: expected.tree,
    schemaCurrent: true,
    commit: "confirmed",
    lifecycleDrain: "confirmed",
    leases: 0,
    equalWork: {
      schema: 1,
      flow: "on",
      workloadHash,
      targetSha256: expected.targetSha256,
      ownerId: id(1),
      memberId: id(2),
      workspaceId: id(3),
      documentId: id(4),
      documentDisplayId: "W-1",
      initialBody: "",
    },
  };
}
test("prepared mode is explicit, remote-only, and malformed flags cannot fall back to fresh setup", () => {
  for (const backend of [undefined, "postgres", "sqlite", "libsql-remote"])
    expect(equalWorkPreparedMode(undefined, backend)).toBe(false);
  expect(equalWorkPreparedMode("1", "libsql-remote")).toBe(true);
  for (const flag of ["", "0", "false", "true", "yes", "01", " 1"])
    expect(() => equalWorkPreparedMode(flag, "libsql-remote")).toThrow();
  for (const backend of [undefined, "postgres", "sqlite", "turso"])
    expect(() => equalWorkPreparedMode("1", backend)).toThrow();
});
test("private binding decodes only the expected prepared allocation and existing logical workload", () => {
  expect(parsePreparedEqualWork(binding(), expected)).toEqual({
    displayId: "W-1",
    allocation: {
      namespace,
      owner: id(1),
      member: id(2),
      workspace: id(3),
      document: id(4),
      logical: {
        users: workload.users,
        documents: workload.documents,
        roles: ["owner", "member"],
        initialBody: "",
        preparation: workload.preparation,
      },
    },
  });
});
const faults: [string, (value: ReturnType<typeof binding>) => void][] = [
  [
    "source",
    (v) => {
      v.source = "f".repeat(40);
    },
  ],
  [
    "tree",
    (v) => {
      v.tree = "f".repeat(40);
    },
  ],
  [
    "target",
    (v) => {
      v.equalWork.targetSha256 = "f".repeat(64);
    },
  ],
  [
    "flow",
    (v) => {
      v.equalWork.flow = "off";
    },
  ],
  [
    "workload",
    (v) => {
      v.equalWork.workloadHash = "f".repeat(64);
    },
  ],
  [
    "namespace",
    (v) => {
      v.namespace = "tui-" + "f".repeat(20);
    },
  ],
  [
    "owner email",
    (v) => {
      v.ownerEmail = expected.memberEmail;
    },
  ],
  [
    "member email",
    (v) => {
      v.memberEmail = expected.ownerEmail;
    },
  ],
  [
    "workspace slug",
    (v) => {
      v.workspaceSlug = "other";
    },
  ],
  [
    "setup needed",
    (v) => {
      v.setupNeeded = true;
    },
  ],
  [
    "unconfirmed commit",
    (v) => {
      v.commit = "not-attempted";
    },
  ],
  [
    "undrained lifecycle",
    (v) => {
      v.lifecycleDrain = "pending";
    },
  ],
  [
    "lease",
    (v) => {
      v.leases = 1;
    },
  ],
  [
    "nonempty document",
    (v) => {
      v.equalWork.initialBody = "history";
    },
  ],
  [
    "missing document",
    (v) => {
      Reflect.deleteProperty(v.equalWork, "documentId");
    },
  ],
  [
    "malformed owner",
    (v) => {
      v.equalWork.ownerId = "missing";
    },
  ],
  [
    "same owner/member",
    (v) => {
      v.equalWork.memberId = v.equalWork.ownerId;
    },
  ],
  [
    "unsafe display ID",
    (v) => {
      v.equalWork.documentDisplayId = "../setup";
    },
  ],
  [
    "extra privileged DTO field",
    (v) => {
      Reflect.set(v.equalWork, "credentials", "forbidden");
    },
  ],
];
for (const [name, fault] of faults)
  test(`${name} refuses prepared allocation`, () => {
    const value = binding();
    fault(value);
    expect(() => parsePreparedEqualWork(value, expected)).toThrow();
  });

const config = resolve(import.meta.dir, "equal-work-playwright.config.ts");
function selected(environment: Record<string, string | undefined>) {
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--no-env-file",
      "--config=" + resolve(import.meta.dir, "../../../bunfig.toml"),
      "--eval",
      `const {default:c}=await import(${JSON.stringify(config)}); console.log(JSON.stringify({metadata:c.metadata,workers:c.workers,retries:c.retries,testMatch:c.testMatch,trace:c.use.trace}));`,
    ],
    {
      env: { CI: "1", FVOCI_E2E_EQUAL_WORK_DIR: "/allocated/markers", ...environment },
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  return { code: result.exitCode, output: result.stdout.toString() };
}
const remoteEnvironment = (path: string) => ({
  FVOCI_E2E_SELECTED_BACKEND: "libsql-remote",
  FVOCI_E2E_SELECTED_FLOW: "on",
  FVOCI_E2E_EQUAL_WORK_PREPARED: "1",
  FVOCI_E2E_TURSO_ACTOR_BINDING: path,
  FVOCI_E2E_TURSO_NAMESPACE: namespace,
  FVOCI_E2E_TURSO_SOURCE: expected.source,
  FVOCI_E2E_TURSO_TREE: expected.tree,
  FVOCI_E2E_TURSO_TARGET_SHA256: expected.targetSha256,
});

test("fresh selected PG/SQLite config retains the same case, one worker, zero retries and reporter purpose", () => {
  for (const backend of ["postgres", "sqlite"])
    for (const flow of ["on", "off"]) {
      const result = selected({
        FVOCI_E2E_SELECTED_BACKEND: backend,
        FVOCI_E2E_SELECTED_FLOW: flow,
      });
      expect(result.code).toBe(0);
      expect(JSON.parse(result.output)).toEqual({
        metadata: { selectedBackend: backend, selectedFlow: flow, purpose: "equal-work" },
        workers: 1,
        retries: 0,
        testMatch: "**/v060-equal-work.spec.ts",
        trace: "off",
      });
    }
});
function withBinding(check: (path: string) => void) {
  const directory = mkdtempSync(join(tmpdir(), "equal-work-prepared-"));
  chmodSync(directory, 0o700);
  const path = join(directory, "actor-binding.json");
  writeFileSync(path, JSON.stringify(binding()), { mode: 0o600, flag: "wx" });
  try {
    check(path);
  } finally {
    rmSync(directory, { recursive: true });
  }
}
test("prepared config accepts the exact owned private binding", () => {
  withBinding((path) => {
    expect(selected(remoteEnvironment(path)).code).toBe(0);
  });
});
for (const name of [
  "FVOCI_E2E_TURSO_TARGET_SHA256",
  "FVOCI_E2E_TURSO_SOURCE",
  "FVOCI_E2E_TURSO_TREE",
  "FVOCI_E2E_TURSO_ACTOR_BINDING",
  "FVOCI_E2E_SELECTED_FLOW",
])
  test("prepared config rejects missing " + name, () => {
    withBinding((path) => {
      expect(selected({ ...remoteEnvironment(path), [name]: undefined }).code).not.toBe(0);
    });
  });
for (const [name, value] of [
  ["FVOCI_E2E_SELECTED_FLOW", "off"],
  ["FVOCI_E2E_TURSO_TARGET_SHA256", "f".repeat(64)],
] as const)
  test("prepared config rejects mismatched " + name, () => {
    withBinding((path) => {
      expect(selected({ ...remoteEnvironment(path), [name]: value }).code).not.toBe(0);
    });
  });
for (const flag of ["", "0", "false"])
  test("prepared config rejects malformed flag " + JSON.stringify(flag), () => {
    withBinding((path) => {
      expect(
        selected({ ...remoteEnvironment(path), FVOCI_E2E_EQUAL_WORK_PREPARED: flag }).code,
      ).not.toBe(0);
    });
  });
test("prepared config rejects a non-private actor binding", () => {
  withBinding((path) => {
    chmodSync(path, 0o644);
    expect(selected(remoteEnvironment(path)).code).not.toBe(0);
  });
});
test("prepared source branch guards setup/create/native member and binds the actual returned browser allocation", () => {
  const source = readFileSync(resolve(import.meta.dir, "v060-equal-work.spec.ts"), "utf8");
  // Direct source contract: side-effecting fresh setup stays behind the same
  // prepared discriminator; actual browser behavior remains runtime NOTRUN.
  assert.match(
    source,
    /if \(!prepared\) \{\s*expect\(\(await page\.request\.get\("\/api\/v1\/setup"\)\)/,
  );
  assert.match(source, /if \(!prepared\) installSelectedMember\(backend\)/);
  assert.match(source, /document\.id\)\.toBe\(prepared\.allocation\.document\)/);
  assert.match(source, /user\.userId\)\.toBe\(prepared\.allocation\.member\)/);
  assert.match(
    source,
    /allocation: \{[\s\S]*owner: owner\.userId,[\s\S]*member: observedMember,[\s\S]*document: document\.id/,
  );
});
