import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";
import type { Inputs } from "../types.ts";
import type { Child, Command, Row } from "./common.ts";
import {
  restartBrowserArgs,
  restartedRoleFacts,
  restartHelper,
  restartSameApp,
  restrictedRoleQuery,
  type RestartContext,
  type Seam,
} from "./restart.ts";

const hash = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");
const tables = [
  "documents",
  "document_states",
  "document_collab_updates",
  "wiki_create_commands",
  "revisions",
];
// The rejection of a promise that must fail, or null when it resolved.
const refusal = (promise: Promise<unknown>) =>
  promise.then(
    () => null,
    (error: unknown) => error as Error,
  );
const facts = () => ({
  user: "owned_app",
  version: "180003",
  superuser: false,
  bypassrls: false,
  owns_schema: false,
  owns_tables: false,
  versions: Array.from({ length: 12 }, (_, i) => i + 1),
  ledger: Array.from({ length: 12 }, (_, i) => [
    i + 1,
    "fvoci-postgres-060",
    (i + 1).toString(16).padStart(64, "0"),
  ]),
  rls: Object.fromEntries(
    tables.map((name) => [
      name,
      { enabled: true, forced: name === "revisions" || name === "wiki_create_commands" },
    ]),
  ),
});
type Facts = ReturnType<typeof facts>;

describe("restarted restricted-role ledger", () => {
  const guard = async (actual: unknown, refused = false) => {
    const calls: string[] = [];
    const pgSql = (sql: string, app: boolean) => {
      expect(app).toBe(true);
      calls.push(sql);
      return refused
        ? Promise.reject(
            Object.assign(new Error("owned app query refused"), { name: "PermissionError" }),
          )
        : Promise.resolve(structuredClone(actual));
    };
    try {
      return await restartedRoleFacts(pgSql, "owned_app", facts());
    } finally {
      expect(calls).toEqual([restrictedRoleQuery]);
    }
  };
  test("the complete facts pass; a reader without the ledger fails", async () => {
    await guard(facts());
    const old: Partial<Facts> = facts();
    delete old.ledger;
    expect(await refusal(guard(old))).toBeInstanceOf(Error);
  });
  test("the query orders the ledger rows by version", () => {
    expect(restrictedRoleQuery).toContain(
      "jsonb_build_array(version,lineage,sql_sha256) ORDER BY version",
    );
  });
  test("missing, changed, reordered and added ledger rows refuse", async () => {
    const changes: ((f: Facts) => void)[] = [
      (f) => {
        (f as Partial<Facts>).ledger = undefined;
      },
      (f) => {
        (f.ledger[0] as unknown[])[1] = "foreign-lineage";
      },
      (f) => {
        (f.ledger[0] as unknown[])[2] = "f".repeat(64);
      },
      (f) => {
        (f.ledger[0] as unknown[])[0] = 99;
      },
      (f) => {
        f.ledger.reverse();
      },
      (f) => {
        f.ledger.push([13, "fvoci-postgres-060", "f".repeat(64)]);
      },
    ];
    for (const change of changes) {
      const changed = facts();
      change(changed);
      expect(await refusal(guard(changed))).toBeInstanceOf(Error);
    }
  });
  test("role, ownership, version and RLS changes refuse", async () => {
    for (const field of [
      "user",
      "version",
      "superuser",
      "bypassrls",
      "owns_schema",
      "owns_tables",
      "versions",
      "rls",
    ] as const) {
      const changed: Record<string, unknown> = facts();
      changed[field] = typeof changed[field] === "boolean" ? true : null;
      expect(await refusal(guard(changed))).toBeInstanceOf(Error);
    }
    const changed = facts();
    (changed.rls.revisions as { forced: boolean }).forced = false;
    expect(await refusal(guard(changed))).toBeInstanceOf(Error);
  });
  test("an app query refusal is not replaced by partial or owner facts", async () => {
    expect((await refusal(guard(facts(), true)))?.message).toBe("owned app query refused");
  });
});

let root: string;
beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "fvoci-restart-"));
});
afterEach(() => {
  rmSync(root, { recursive: true, force: true });
});
const cliContext = () => {
  const cli = join(root, "node_modules/playwright/cli.js");
  mkdirSync(join(root, "node_modules/playwright"), { recursive: true });
  writeFileSync(cli, "synthetic qualified CLI, never executed");
  return {
    cli,
    context: {
      root,
      bun: join(root, "bun"),
      spec: "workspace-wiki-selected-backend.spec.ts",
      playwrightCli: cli,
      before: { external: { [cli]: hash(cli) } },
    },
  };
};

describe("restart browser CLI", () => {
  test("same admitted direct args, with or without the explicit CLI path", () => {
    const { cli, context } = cliContext();
    const expected = [
      join(root, "bun"),
      "--no-install",
      cli,
      "test",
      "--config",
      "e2e-pending/collab-playwright.config.ts",
      "--reporter=line,json",
      "--grep",
      "selected normal main restart:",
      "workspace-wiki-selected-backend.spec.ts",
    ];
    expect(restartBrowserArgs(context)).toEqual(expected);
    expect(restartBrowserArgs({ ...context, playwrightCli: undefined })).toEqual(expected);
  });
  test("missing, unadmitted, drifted, symlinked and foreign CLIs refuse", () => {
    const { cli, context } = cliContext();
    const original = readFileSync(cli, "utf8");
    expect(() => restartBrowserArgs({ ...context, before: { external: {} } })).toThrow();
    writeFileSync(cli, "drift");
    expect(() => restartBrowserArgs(context)).toThrow();
    unlinkSync(cli);
    expect(() => restartBrowserArgs(context)).toThrow();
    const foreign = join(root, "foreign");
    writeFileSync(foreign, original);
    symlinkSync(foreign, cli);
    expect(() => restartBrowserArgs(context)).toThrow();
    expect(() => restartBrowserArgs({ ...context, playwrightCli: foreign })).toThrow();
  });
});

const secret = "SYNTHETIC_PRIVATE_SECRET http://private.invalid/x token=PRIVATE";
const restartTitle =
  "selected normal main restart: fresh actor reads persisted native history and manual revision";
interface Probe {
  report?: "structured" | "missing" | "malformed" | "readback-without-history";
  beforeBrowser?: boolean;
  cleanupFailure?: boolean;
  cliMissing?: boolean;
}
// A one-test passing Playwright JSON report carrying one JSON attachment.
const passedReport = (name: string, body: unknown) => ({
  config: { workers: 1 },
  errors: [],
  stats: { expected: 1, unexpected: 0, flaky: 0, skipped: 0 },
  suites: [
    {
      specs: [
        {
          tests: [
            {
              results: [
                {
                  status: "passed",
                  retry: 0,
                  errors: [],
                  attachments: [
                    {
                      name,
                      contentType: "application/json",
                      body: Buffer.from(JSON.stringify(body)).toString("base64"),
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    },
  ],
});
const exited = (code: number): Child => ({
  exited: Promise.resolve(code),
  exitCode: code,
  signalCode: null,
  kill: () => undefined,
});

// The restart helper with its process, Docker, HTTP and DB I/O replaced; the
// allocation, files, report and receipt are real.
async function failureProbe(options: Probe = {}) {
  const report = options.report ?? "structured";
  const run = join(root, "run"),
    storage = join(root, "storage"),
    dist = join(root, "dist");
  for (const directory of [run, storage, dist]) mkdirSync(directory);
  const { cli, context: cliFacts } = cliContext();
  const parent = join(root, "parent.ts"),
    chromium = join(root, "chromium");
  writeFileSync(parent, "synthetic parent");
  writeFileSync(join(root, "bun"), "synthetic bun");
  writeFileSync(chromium, "synthetic browser");
  writeFileSync(join(run, "source-inputs-before.json"), "{}");
  const binaries: Record<string, { sha256: string }> = {};
  const paths: Record<string, string> = {};
  for (const label of ["server", "migrate", "engine"]) {
    const path = join(root, label);
    writeFileSync(path, label);
    paths[label] = path;
    binaries[path] = { sha256: hash(path) };
  }
  const browserInputs = {
    bun: { path: join(root, "bun"), sha256: hash(join(root, "bun")) },
    chromium: { path: chromium, sha256: hash(chromium) },
    chromium_directory_files: {},
  };
  const initial = facts();
  const binding = {
    runId: "123",
    runAttempt: "1",
    source: "a".repeat(40),
    tree: "b".repeat(40),
    compiledSource: "a".repeat(40),
    backend: "postgres",
    runRoot: run,
    parentDriverSha256: hash(parent),
    restartHelperSha256: hash(restartHelper),
    sourceInputsSha256: hash(join(run, "source-inputs-before.json")),
    artifactHashes: Object.fromEntries(Object.entries(binaries).map(([p, r]) => [p, r.sha256])),
    assetHashes: {},
    browserInputs,
    abiHashes: {},
  };
  const grant = join(root, "grant.json");
  writeFileSync(
    grant,
    JSON.stringify({
      schema: 1,
      status: "GRANTED",
      owner: "pure-owner",
      exclusiveCIJob: true,
      currentCIJobConfirmed: true,
      runId: "123",
      runAttempt: "1",
      source: "a".repeat(40),
      compiledSource: "a".repeat(40),
      tree: "b".repeat(40),
      backend: "postgres",
      binding,
    }),
  );
  const seed = {
    selected: "postgres",
    firstAck: "first",
    finalAck: "final",
    creatorId: "creator",
    freshActorId: "fresh",
    workspaceId: "workspace",
    document: { id: "document" },
    canonicalEmojiOracleControls: [0, 1, 2, 3, 4, 5],
    nativeHistoryOracleControls: [0, 1],
  };
  writeFileSync(
    join(run, "playwright-result.private.json"),
    JSON.stringify({
      config: { workers: 1 },
      errors: [],
      stats: { expected: 1, unexpected: 0, flaky: 0, skipped: 0 },
      suites: [
        {
          specs: [
            {
              tests: [
                {
                  results: [
                    {
                      status: "passed",
                      retry: 0,
                      errors: [],
                      attachments: [
                        {
                          name: "selected-vue-native-readback.json",
                          contentType: "application/json",
                          body: Buffer.from(JSON.stringify(seed)).toString("base64"),
                        },
                      ],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    }),
  );
  const old: Row = {
    pid: 11,
    parent: 1,
    start_ticks: "1",
    namespace_pid: 11,
    uid: 1000,
    gid: 1000,
    args: "/fvoci/bin/fvoci-server",
  };
  const restarted: Row = { ...old, pid: 22, start_ticks: "2", namespace_pid: 22 };
  const rows = [[old], [restarted], [restarted]];
  let restartWaits = 0,
    spawned = 0;
  const observed: string[][] = [];
  const command: Command = (args, commandOptions = {}) => {
    observed.push(args);
    let stdout = "";
    if (args[0] === join(root, "bun")) {
      expect(args).toEqual(restartBrowserArgs(cliFacts));
      expect(JSON.stringify(commandOptions.env)).not.toContain(secret);
      writeFileSync(commandOptions.log as string, secret);
      const path = join(run, "restart-playwright-result.private.json");
      if (report === "structured")
        writeFileSync(
          path,
          JSON.stringify({
            config: { workers: 1 },
            suites: [
              {
                specs: [
                  {
                    title: restartTitle,
                    file: "e2e-pending/workspace-wiki-selected-backend.spec.ts",
                    tests: [
                      {
                        results: [
                          {
                            status: "failed",
                            errorLocation: {
                              file: "/fixed/apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts",
                              line: 701,
                            },
                            errors: [{ message: secret, stack: secret }],
                          },
                        ],
                      },
                    ],
                  },
                ],
              },
            ],
          }),
        );
      else if (report === "malformed") writeFileSync(path, secret);
      else if (report === "readback-without-history") {
        // A passing restart run whose readback and seed both lack persisted
        // and revision: equal absences are not a readback.
        const readback = {
          ...seed,
          source: "a".repeat(40),
          tree: "b".repeat(40),
          documentId: "document",
        };
        writeFileSync(
          path,
          JSON.stringify(passedReport("selected-vue-restart-readback.json", readback)),
        );
        return Promise.resolve({ returncode: 0, stdout: "", stderr: "" });
      }
      return Promise.resolve({ returncode: 7, stdout: "", stderr: "" });
    }
    if (args.includes("sha256sum"))
      stdout = ["server", "migrate", "engine"]
        .map((n) => (binaries[paths[n] as string]?.sha256 ?? "") + "  fixture")
        .join("\n");
    else if (args.includes("inspect")) stdout = "pure-owner";
    else if (args.includes("stat")) stdout = "0 1000 640";
    else if (args.includes("cat")) stdout = "synthetic_scoped_key_long_enough";
    return Promise.resolve({ returncode: 0, stdout, stderr: "" });
  };
  const emitted: string[] = [];
  const seam: Seam = {
    command,
    ownedRows: () => Promise.resolve(rows.shift() ?? []),
    identityGone: () => true,
    pgSql: () => {
      const actual = facts();
      if (options.beforeBrowser) actual.superuser = true;
      return Promise.resolve(actual);
    },
    spawnServer: (_args, log) => {
      spawned += 1;
      writeSync(
        log,
        "fvoci-server listening on http://127.0.0.1:456\nmeilisearch enabled\noutbox dispatcher started\n",
      );
      return {
        get exited() {
          restartWaits += 1;
          return options.cleanupFailure
            ? Promise.reject(
                Object.assign(new Error("synthetic-owned-exec"), { name: "TimeoutExpired" }),
              )
            : Promise.resolve(0);
        },
        exitCode: options.cleanupFailure ? null : 0,
        signalCode: null,
        kill: () => undefined,
      };
    },
    probeSetup: () => Promise.resolve({ status: 200, body: { needed: false } }),
    portClosed: () => Promise.resolve(true),
    emit: (line) => emitted.push(line),
    actor: () => [1000, 1000],
  };
  const laneReceipt: Record<string, unknown> = { actual_restricted_role_schema_rls: initial };
  const context: RestartContext = {
    run,
    name: "pure-container",
    browserEnv: {
      FVOCI_E2E_SELECTED_BACKEND: "postgres",
      PATH: "/usr/bin",
      LANG: "C.UTF-8",
      PRIVATE_TOKEN: secret,
    },
    code: 0,
    head: "a".repeat(40),
    tree: "b".repeat(40),
    compiledHead: "a".repeat(40),
    identity: { runId: "123", runAttempt: "1" },
    parentDriver: parent,
    binaries,
    server: paths.server as string,
    migrate: paths.migrate as string,
    engine: paths.engine as string,
    distFiles: {},
    abiFiles: {},
    browserInputs,
    root,
    bun: join(root, "bun"),
    spec: "workspace-wiki-selected-backend.spec.ts",
    playwrightCli: cli,
    before: { external: cliFacts.before.external } as unknown as Inputs,
    sourceBefore: {} as Inputs,
    inputCheck: () => Promise.resolve({} as Inputs),
    treeHashes: () => ({}),
    dist,
    storage,
    serverRow: old,
    serverProcess: exited(0),
    base: "http://127.0.0.1:123",
    receipt: laneReceipt,
    projectBrowser: true,
    role: "owned_app",
    masterKey: "different synthetic owner key",
  };
  if (options.cliMissing) unlinkSync(cli);
  const saved = { ...process.env };
  Object.assign(process.env, {
    FVOCI_CI_OWNER: "pure-owner",
    FVOCI_ROOT_RUN_OWNER: "pure-owner",
    FVOCI_ROOT_RESTART_GRANT: grant,
  });
  let caught: unknown;
  try {
    await restartSameApp(context, seam);
  } catch (error) {
    caught = error;
  } finally {
    for (const key of ["FVOCI_CI_OWNER", "FVOCI_ROOT_RUN_OWNER", "FVOCI_ROOT_RESTART_GRANT"])
      if (saved[key] === undefined) Reflect.deleteProperty(process.env, key);
      else process.env[key] = saved[key];
  }
  expect((caught as Error).name).toBe("AssertionError");
  const receiptPath = join(run, "restart-receipt.private.json");
  const receipt = JSON.parse(readFileSync(receiptPath, "utf8")) as Record<string, unknown>;
  expect((receipt.originalFailure as { message: string }).message).toBe((caught as Error).message);
  expect(emitted.length).toBe(1);
  expect(emitted.join("")).not.toContain("PRIVATE");
  const safe = JSON.parse(emitted[0] as string) as Record<string, unknown>;
  expect(safe.restart_helper_checkpoint).toMatch(
    /^tools\/selected-backend-ci\/drivers\/restart\.ts:[1-9][0-9]*$/,
  );
  expect(statSync(receiptPath).mode & 0o777).toBe(0o600);
  if (options.cliMissing) {
    expect(spawned).toBe(0);
    expect(observed).toEqual([]);
  } else {
    expect(restartWaits).toBe(1);
    expect(observed.some((args) => args.includes("/bin/kill") && args.includes("22"))).toBe(true);
    if (options.cleanupFailure) expect("restartServerExit" in receipt).toBe(false);
    else expect(receipt.restartServerExit).toBe(0);
  }
  return { safe, receipt, laneReceipt };
}

describe("restart failure and cleanup", () => {
  test("browser first failure keeps the structured checkpoint and cleans up normally", async () => {
    const { safe, receipt, laneReceipt } = await failureProbe();
    expect(safe.restart_browser_exit).toBe(7);
    expect(safe.known_browser_status).toBe("failed");
    expect(safe.known_browser_checkpoint).toBe(
      "e2e-pending/workspace-wiki-selected-backend.spec.ts:701",
    );
    expect(laneReceipt.known_browser_checkpoint).toBe(safe.known_browser_checkpoint);
    expect((receipt.originalFailure as { message: string }).message).toBe(
      "preserve original restart browser failure",
    );
    expect(receipt.cleanupErrors).toEqual([]);
  });
  test("an absent report stays missing", async () => {
    const { safe } = await failureProbe({ report: "missing" });
    expect(safe.browser_report_state).toBe("report-missing");
    expect(safe.known_browser_checkpoint).toBeNull();
  });
  test("a malformed report keeps the first failure", async () => {
    const { safe, receipt } = await failureProbe({ report: "malformed" });
    expect(safe.browser_report_state).toBe("report-unreadable");
    expect((receipt.originalFailure as { message: string }).message).toBe(
      "preserve original restart browser failure",
    );
    expect(receipt.cleanupErrors).toEqual([]);
  });
  test("a failure before the browser is the helper checkpoint, not the old cause", async () => {
    const { safe } = await failureProbe({ beforeBrowser: true });
    expect(safe.restart_browser_exit).toBeNull();
    expect(safe.known_browser_checkpoint).toBeNull();
    expect(safe.restart_stage).toBe("validated");
  });
  test("absent persisted and revision on both sides are not a readback", async () => {
    const { safe, receipt } = await failureProbe({ report: "readback-without-history" });
    expect(safe.restart_browser_exit).toBe(0);
    expect((receipt.originalFailure as { message: string }).message).toBe(
      "required JSON member missing",
    );
    expect(receipt.stage).toBe("restarted normal main ready");
    expect(receipt.cleanupErrors).toEqual([]);
  });
  test("a CLI refusal precedes any restart child", async () => {
    await failureProbe({ cliMissing: true });
  });
  test("a cleanup failure does not replace the original browser failure", async () => {
    const { safe, receipt } = await failureProbe({ cleanupFailure: true });
    expect((receipt.originalFailure as { message: string }).message).toBe(
      "preserve original restart browser failure",
    );
    expect((receipt.cleanupErrors as unknown[]).length).toBe(1);
    expect(safe.restart_browser_exit).toBe(7);
  });
  test("a foreign actor is refused before any resource", async () => {
    const calls: string[] = [];
    const seam = new Proxy(
      { actor: () => [1001, 1000] as const },
      {
        get: (target, key) => {
          calls.push(String(key));
          return Reflect.get(target, key) as unknown;
        },
      },
    ) as unknown as Seam;
    const saved = process.env.FVOCI_CI_OWNER;
    process.env.FVOCI_CI_OWNER = "pure-owner";
    try {
      const refused = await restartSameApp(
        { browserEnv: { FVOCI_E2E_SELECTED_BACKEND: "postgres" } } as unknown as RestartContext,
        seam,
      ).catch((error: unknown) => error);
      expect((refused as Error).name).toBe("AssertionError");
    } finally {
      if (saved === undefined) Reflect.deleteProperty(process.env, "FVOCI_CI_OWNER");
      else process.env.FVOCI_CI_OWNER = saved;
    }
    expect(calls).toEqual(["actor"]);
  });
});
