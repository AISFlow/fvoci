import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "bun:test";

const HERE = dirname(fileURLToPath(import.meta.url));
const TS = join(HERE, "compare-catalogs.ts");
// Fixed outcomes of the Python original. Each case was captured once by running
// compare-catalogs.py (CPython 3.14.4) at commit
// 431c7f91745edb56a21bd0643ff100739cd76295 on exactly the inputs this file
// writes. A case is keyed by the SHA-256 of its argv and input files, so a
// changed input finds no captured outcome and fails instead of comparing
// against a stale one. "<DIR>" stands for the per-case temporary directory.
// report is null when no report file was written and true when the report
// file bytes equal stdout.
const ORACLE_PATH = join(HERE, "fixtures", "compare-catalogs-python-oracle.json");
const OWNER = "fvoci_owner";
const APP = "fvoci_app_cmp";
const NEW_COLS = ["version", "lineage", "sql_sha256", "applied_at"];
const OLD_COLS = ["version", "applied_at"];

function acl(entries: string[]) {
  return `{${entries.join(",")}}`;
}

const LEDGER_ACL = acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/${OWNER}`]);
const USERS_ACL = acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=arwd/${OWNER}`]);

type Column = {
  num: number;
  name: string;
  type: string;
  notnull: boolean;
  default: string | null;
  identity: string;
  generated: string;
  collation: string;
  acl: string | null;
};
type Catalog = {
  server_version: string;
  schemas: Array<{ name: string; acl: null }>;
  tables: Array<Record<string, unknown>>;
  sequences: unknown[];
  functions: unknown[];
  views: unknown[];
  extensions: unknown[];
  seeds: Record<string, unknown>;
  ledger: Array<Record<string, unknown>>;
  app_role?: Record<string, unknown>;
};

function table(
  name: string,
  columns: string[],
  constraints: Array<[string, string]> = [],
  indexes: Array<[string, string]> = [],
  tableAcl: string | null = null,
) {
  return {
    name,
    kind: "r",
    rls: true,
    force_rls: true,
    acl: tableAcl,
    columns: columns.map((column, index): Column => ({
      num: index + 1,
      name: column,
      type: "text",
      notnull: true,
      default: null,
      identity: "",
      generated: "",
      collation: "-",
      acl: null,
    })),
    constraints: constraints.map(([constraint, def]) => ({
      name: constraint,
      type: "c",
      def,
      deferrable: false,
      deferred: false,
      validated: true,
    })),
    indexes: indexes.map(([index, def]) => ({
      name: index,
      def,
      unique: false,
      primary: false,
      valid: true,
    })),
    triggers: [],
    policies: [],
  };
}

function catalog(
  ledgerColumns: string[],
  ledgerRows: Array<Record<string, unknown>>,
  usersDefault = "now()",
): Catalog {
  const users = table("users", ["id", "email"], [], [], USERS_ACL);
  users.columns[1].default = usersDefault;
  return {
    server_version: "18.0",
    schemas: [{ name: "fvoci", acl: null }],
    tables: [
      table(
        "schema_migrations",
        ledgerColumns,
        [["schema_migrations_pkey", "PRIMARY KEY (version)"]],
        [],
        LEDGER_ACL,
      ),
      users,
    ],
    sequences: [],
    functions: [],
    views: [],
    extensions: ["plpgsql"],
    seeds: {
      instance_settings_meta: [{ id: 1, revision: 0 }],
      instance_config: [{ id: 1 }],
      outbox_consumers: [],
      row_counts: { users: 0 },
    },
    ledger: ledgerRows,
  };
}

function tp(
  tableName: string,
  privilege: string,
  grantor = OWNER,
  grantable = "NO",
  schema = "fvoci",
) {
  return { schema, table: tableName, privilege, grantor, grantable };
}

function cp(
  tableName: string,
  column: string,
  privilege: string,
  grantor = OWNER,
  grantable = "NO",
  schema = "fvoci",
) {
  return { schema, table: tableName, column, privilege, grantor, grantable };
}

function attributes(overrides: Record<string, unknown> = {}) {
  return {
    exists: true,
    superuser: false,
    inherit: true,
    createrole: false,
    createdb: false,
    login: true,
    replication: false,
    bypassrls: false,
    member_of: [] as unknown[],
    ...overrides,
  };
}

function withRole(
  cat: Catalog,
  ledgerColumns: string[],
  options: {
    role?: string;
    grantor?: string;
    ledgerGrantable?: string;
    extraTable?: Array<ReturnType<typeof tp>>;
    extraColumn?: Array<ReturnType<typeof cp>>;
    dropColumn?: Array<[string, string, string]>;
    attrs?: ReturnType<typeof attributes>;
  } = {},
) {
  const next = structuredClone(cat);
  const grantor = options.grantor ?? OWNER;
  const ledgerGrantable = options.ledgerGrantable ?? "NO";
  let tablePrivileges = [
    tp("schema_migrations", "SELECT", grantor, ledgerGrantable),
    tp("users", "SELECT", grantor),
    tp("users", "INSERT", grantor),
  ];
  let columnPrivileges = ledgerColumns.map((column) =>
    cp("schema_migrations", column, "SELECT", grantor, ledgerGrantable),
  );
  for (const column of ["id", "email"])
    for (const privilege of ["SELECT", "INSERT"])
      columnPrivileges.push(cp("users", column, privilege, grantor));
  tablePrivileges = tablePrivileges.concat(options.extraTable ?? []);
  columnPrivileges = columnPrivileges.concat(options.extraColumn ?? []);
  const dropped = new Set((options.dropColumn ?? []).map((item) => item.join("\0")));
  columnPrivileges = columnPrivileges.filter(
    (item) => !dropped.has([item.table, item.column, item.privilege].join("\0")),
  );
  next.app_role = {
    role: options.role ?? APP,
    attributes: options.attrs ?? attributes(),
    schema_privileges: {
      fvoci: { usage: true, create: false },
      public: { usage: true, create: false },
    },
    table_privileges: tablePrivileges,
    column_privileges: columnPrivileges,
    routine_privileges: [{ routine: "fvoci.app_now()", execute: true }],
    sequence_usage: true,
    schema_usage: true,
  };
  return next;
}

type Outcome = { status: number; stdout: string; stderr: string; report: string | true | null };
type Files = Record<string, string | Uint8Array>;
type OracleFile = { source: string; cases: Record<string, Outcome & { label: string }> };

const ORACLE = JSON.parse(readFileSync(ORACLE_PATH, "utf8")) as OracleFile;
const usedCases = new Set<string>();
let currentLabel = "";
let labelCount = 0;

function caseTest(name: string, body: () => void) {
  test(name, () => {
    currentLabel = name;
    labelCount = 0;
    body();
  });
}

function caseKey(args: string[], files: Files) {
  const hash = createHash("sha256");
  hash.update(JSON.stringify(args));
  for (const name of Object.keys(files).sort()) {
    hash.update(`\0${name}\0`);
    hash.update(files[name]);
  }
  return hash.digest("hex");
}

function expectedFor(args: string[], files: Files): Outcome {
  const key = caseKey(args, files);
  const found: (Outcome & { label: string }) | undefined = ORACLE.cases[key];
  if (!found)
    throw new Error(
      `no captured compare-catalogs.py outcome for ${currentLabel} #${String(labelCount)} (${key})`,
    );
  usedCases.add(key);
  return { status: found.status, stdout: found.stdout, stderr: found.stderr, report: found.report };
}

function decode(bytes: Uint8Array) {
  const text = Buffer.from(bytes).toString("utf8");
  expect(Buffer.from(text, "utf8").equals(Buffer.from(bytes))).toBe(true);
  return text;
}

// Runs compare-catalogs.ts with args in which "<DIR>" names a fresh directory
// holding files, and returns its outcome beside the captured Python outcome.
function invoke(files: Files, args: string[]) {
  labelCount += 1;
  const dir = mkdtempSync(join(tmpdir(), "compare-catalogs-"));
  try {
    for (const [name, body] of Object.entries(files)) writeFileSync(join(dir, name), body);
    const argv = args.map((arg) => arg.replaceAll("<DIR>", dir));
    const reportAt = argv.indexOf("--report");
    const reportPath = reportAt >= 0 ? argv[reportAt + 1] : undefined;
    const py = expectedFor(args, files); // ORACLE-LOOKUP
    const proc = spawnSync(process.execPath, [TS, ...argv]);
    expect(proc.status).not.toBeNull();
    const normalize = (text: string) => text.replaceAll(dir, "<DIR>");
    const stdout = normalize(decode(proc.stdout));
    const reportText =
      reportPath !== undefined && existsSync(reportPath)
        ? normalize(decode(readFileSync(reportPath)))
        : null;
    const ts: Outcome = {
      status: proc.status ?? -1,
      stdout,
      stderr: normalize(decode(proc.stderr)),
      report: reportText !== null && reportText === stdout ? true : reportText,
    };
    return { ts, py };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

// The last line of a CPython traceback: the exception and its message.
function exceptionLine(stderr: string) {
  return `${stderr.trimEnd().split("\n").at(-1) ?? ""}\n`;
}

const REPORTED = ["<DIR>/a.json", "<DIR>/b.json", "--report", "<DIR>/report.md"];
const UNREPORTED = ["<DIR>/a.json", "<DIR>/b.json"];

function runRaw(oldText: string, newText: string) {
  const { ts, py } = invoke({ "a.json": oldText, "b.json": newText }, REPORTED);
  expect(ts).toEqual(py);
  expect(ts.report).toBe(true);
  return { rc: ts.status, out: ts.stdout };
}

function run(old: unknown, fresh: unknown) {
  return runRaw(JSON.stringify(old), JSON.stringify(fresh));
}

function normalizeProg(text: string) {
  return text.replaceAll("compare-catalogs.py", "PROG").replaceAll("compare-catalogs.ts", "PROG");
}

const OLD_BARE = catalog(
  OLD_COLS,
  Array.from({ length: 55 }, (_, index) => ({ version: index + 1 })),
);
const NEW_BARE = catalog(
  NEW_COLS,
  Array.from({ length: 12 }, (_, index) => ({
    version: index + 1,
    lineage: "fvoci-postgres-060",
    sql_sha256: (index + 1).toString(16).padStart(64, "0"),
  })),
);
const OLD_LEDGER = withRole(OLD_BARE, OLD_COLS);
const NEW_LEDGER = withRole(NEW_BARE, NEW_COLS);
const OLD_ROLE = withRole(OLD_BARE, OLD_COLS);
const NEW_ROLE = withRole(NEW_BARE, NEW_COLS);

function diverge(old: unknown, fresh: unknown) {
  return invoke({ "a.json": JSON.stringify(old), "b.json": JSON.stringify(fresh) }, UNREPORTED);
}

function ledgerTable(cat: Catalog) {
  const found = cat.tables.find((item) => item.name === "schema_migrations");
  if (!found) throw new Error("missing ledger");
  return found;
}

caseTest("ledger table and rows are the only declared exception", () => {
  const { rc, out } = run(OLD_LEDGER, NEW_LEDGER);
  expect(rc).toBe(0);
  expect(out).toContain("RESULT: PASS");
  expect(out).toContain("LEDGER TABLE (declared exception");
  expect(out).toContain("LEDGER ROWS (declared exception");
  expect(out).not.toContain("DIFF table schema_migrations");
});

caseTest("a product column default difference fails", () => {
  const fresh = structuredClone(NEW_LEDGER);
  (fresh.tables[1].columns as Column[])[1].default = "'x'::text";
  const { rc, out } = run(OLD_LEDGER, fresh);
  expect(rc).toBe(1);
  expect(out).toContain("DIFF table users");
  expect(out).toContain("RESULT: FAIL");
});

caseTest("a missing product constraint or index fails", () => {
  const cases = [
    [
      "constraints",
      {
        name: "users_email_unique",
        type: "u",
        def: "UNIQUE (email)",
        deferrable: false,
        deferred: false,
        validated: true,
      },
    ],
    [
      "indexes",
      {
        name: "users_email_idx",
        def: "CREATE INDEX users_email_idx ON fvoci.users USING btree (email)",
        unique: false,
        primary: false,
        valid: true,
      },
    ],
  ] as const;
  for (const [key, item] of cases) {
    const old = structuredClone(OLD_LEDGER);
    (old.tables[1][key] as unknown[]).push(item);
    const { rc, out } = run(old, NEW_LEDGER);
    expect(rc).toBe(1);
    expect(out).toContain(`MISSING table users.${key} ${item.name}`);
  }
});

caseTest("malformed new ledger is a failure not an exception", () => {
  const cases: Array<[string, (cat: Catalog) => void]> = [
    [
      "wrong lineage",
      (cat) => {
        cat.ledger[3] = { ...cat.ledger[3], lineage: "fvoci-postgres-999" };
      },
    ],
    [
      "missing digest",
      (cat) => {
        cat.ledger[5] = { version: 6, lineage: "fvoci-postgres-060" };
      },
    ],
    [
      "short digest",
      (cat) => {
        cat.ledger[1] = { ...cat.ledger[1], sql_sha256: "abc" };
      },
    ],
    [
      "gap in versions",
      (cat) => {
        cat.ledger.splice(4, 1);
      },
    ],
    [
      "duplicate digest",
      (cat) => {
        cat.ledger[2] = { ...cat.ledger[2], sql_sha256: cat.ledger[1].sql_sha256 };
      },
    ],
    [
      "extra ledger column",
      (cat) => {
        (ledgerTable(cat).columns as Column[]).push({
          num: 5,
          name: "note",
          type: "text",
          notnull: false,
          default: null,
          identity: "",
          generated: "",
          collation: "-",
          acl: null,
        });
      },
    ],
    [
      "nullable digest column",
      (cat) => {
        (ledgerTable(cat).columns as Column[])[2].notnull = false;
      },
    ],
    [
      "no rows",
      (cat) => {
        cat.ledger = [];
      },
    ],
  ];
  for (const [label, mutate] of cases) {
    const fresh = structuredClone(NEW_LEDGER);
    mutate(fresh);
    const { rc, out } = run(OLD_LEDGER, fresh);
    expect(rc, label).toBe(1);
    expect(out, label).toContain("LEDGER");
    expect(out, label).toContain("RESULT: FAIL");
  }
  const old = structuredClone(OLD_LEDGER);
  old.ledger = [{ version: 1, lineage: "fvoci-postgres-060", sql_sha256: "0".repeat(64) }];
  const { rc, out } = run(old, NEW_LEDGER);
  expect(rc).toBe(1);
  expect(out).toContain("old receipt 1 carries a lineage");
});

caseTest("a missing ledger table is reported not silently excepted", () => {
  const fresh = structuredClone(NEW_LEDGER);
  fresh.tables = fresh.tables.filter((item) => item.name !== "schema_migrations");
  const { rc, out } = run(OLD_LEDGER, fresh);
  expect(rc).toBe(1);
  expect(out).toContain("MISSING ledger table schema_migrations");
});

caseTest("seed and column order differences fail", () => {
  const seeded = structuredClone(NEW_LEDGER);
  seeded.seeds.instance_settings_meta = [{ id: 1, revision: 1 }];
  let result = run(OLD_LEDGER, seeded);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF seed instance_settings_meta");
  const reordered = structuredClone(NEW_LEDGER);
  (reordered.tables[1].columns as Column[]).reverse();
  (reordered.tables[1].columns as Column[]).forEach((column, index) => {
    column.num = index + 1;
  });
  result = run(OLD_LEDGER, reordered);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("COLUMN-ORDER table users");
});

caseTest("role inclusive ledger select expansion passes", () => {
  const { rc, out } = run(OLD_ROLE, NEW_ROLE);
  expect(rc).toBe(0);
  expect(out).toContain("RESULT: PASS");
  const flagged = out
    .split("\n")
    .filter(
      (line) => line.startsWith("EXTRA") || line.startsWith("MISSING") || line.startsWith("DIFF"),
    )
    .join("\n");
  expect(flagged).not.toContain("app_role");
});

caseTest("a missing app_role or an unlabeled ledger acl fails closed", () => {
  const elevated = withRole(OLD_BARE, OLD_COLS, { attrs: attributes({ superuser: true }) });
  let result = diverge(elevated, NEW_BARE);
  expect(result.py.status).toBe(0);
  expect(result.ts.status).toBe(1);
  expect(result.ts.stdout).toContain("RESULT: FAIL");
  expect(result.ts.stdout).toContain("app_role missing on new");
  expect(result.ts.stdout).not.toContain("NOTE app_role");

  const attacked = acl(["owner=arwdDxtm/owner", "app=r/owner", "attacker=r/owner"]);
  const oldAttack = structuredClone(OLD_BARE);
  const newAttack = structuredClone(NEW_BARE);
  ledgerTable(oldAttack).acl = attacked;
  ledgerTable(newAttack).acl = attacked;
  result = diverge(oldAttack, newAttack);
  expect(result.py.status).toBe(0);
  expect(result.ts.status).toBe(1);
  expect(result.ts.stdout).toContain("RESULT: FAIL");
  expect(result.ts.stdout).toContain("foreign grantee 'attacker'");
  expect(result.ts.stdout).toContain("app_role missing on old");
  expect(result.ts.stdout).toContain("app_role missing on new");

  const appOwner = acl(["app=arwdDxtm/app"]);
  const oldOwner = structuredClone(OLD_BARE);
  const newOwner = structuredClone(NEW_BARE);
  ledgerTable(oldOwner).acl = appOwner;
  ledgerTable(newOwner).acl = appOwner;
  result = diverge(oldOwner, newOwner);
  expect(result.py.status).toBe(0);
  expect(result.ts.status).toBe(1);
  expect(result.ts.stdout).toContain("RESULT: FAIL");
  expect(result.ts.stdout).toContain("app role must not hold owner write");
  expect(result.ts.stdout).toContain("'arwdDxtm'");
});

caseTest("unauthorized or incomplete ledger privileges fail", () => {
  const cases: Record<string, Catalog> = {
    "new INSERT on ledger table": withRole(NEW_LEDGER, NEW_COLS, {
      extraTable: [tp("schema_migrations", "INSERT")],
    }),
    "new UPDATE on lineage column": withRole(NEW_LEDGER, NEW_COLS, {
      extraColumn: [cp("schema_migrations", "lineage", "UPDATE")],
    }),
    "new DELETE on ledger table": withRole(NEW_LEDGER, NEW_COLS, {
      extraTable: [tp("schema_migrations", "DELETE")],
    }),
    "new SELECT missing on sql_sha256": withRole(NEW_LEDGER, ["version", "lineage", "applied_at"]),
    "new SELECT on a column the ledger does not have": withRole(
      NEW_LEDGER,
      NEW_COLS.concat(["note"]),
    ),
    "new table SELECT missing": withRole(NEW_LEDGER, NEW_COLS),
    "new ledger SELECT grantable (table and columns)": withRole(NEW_LEDGER, NEW_COLS, {
      ledgerGrantable: "YES",
    }),
    "new ledger column SELECT grantable only": withRole(NEW_LEDGER, NEW_COLS, {
      extraColumn: [cp("schema_migrations", "version", "SELECT", OWNER, "YES")],
      dropColumn: [["schema_migrations", "version", "SELECT"]],
    }),
    "new ledger grant from a second grantor": withRole(NEW_LEDGER, NEW_COLS, {
      extraTable: [tp("schema_migrations", "SELECT", "other_admin")],
    }),
  };
  const role = cases["new table SELECT missing"].app_role;
  if (!role) throw new Error("missing role");
  role.table_privileges = (role.table_privileges as Array<{ table: string }>).filter(
    (item) => item.table !== "schema_migrations",
  );
  for (const [label, fresh] of Object.entries(cases)) {
    const { rc, out } = run(OLD_ROLE, fresh);
    expect(rc, label).toBe(1);
    expect(out, label).toContain("LEDGER app_role new");
  }
  let old = withRole(OLD_LEDGER, OLD_COLS.concat(["lineage"]));
  let result = run(old, NEW_ROLE);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("LEDGER app_role old column privileges");
  old = withRole(OLD_LEDGER, OLD_COLS, { grantor: "owner_a" });
  const fresh = withRole(NEW_LEDGER, NEW_COLS, { grantor: "owner_b" });
  result = run(old, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("LEDGER grantor differs across sides");
});

caseTest("non ledger grant differences still fail", () => {
  let fresh = withRole(NEW_LEDGER, NEW_COLS, { extraColumn: [cp("users", "email", "UPDATE")] });
  let result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("EXTRA app_role.column_privileges");
  expect(result.out).toContain('"privilege": "UPDATE"');
  fresh = withRole(NEW_LEDGER, NEW_COLS, { dropColumn: [["users", "id", "SELECT"]] });
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("MISSING app_role.column_privileges");
  fresh = withRole(NEW_LEDGER, NEW_COLS, {
    extraTable: [tp("users", "SELECT", OWNER, "NO", "public")],
  });
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("EXTRA app_role.table_privileges");
  expect(result.out).toContain('"schema": "public"');
  fresh = withRole(NEW_LEDGER, NEW_COLS, { extraTable: [tp("users", "SELECT", OWNER, "YES")] });
  const privileges = fresh.app_role?.table_privileges as Array<{
    table: string;
    privilege: string;
    grantable: string;
  }>;
  fresh.app_role!.table_privileges = privileges.filter(
    (item) => !(item.table === "users" && item.privilege === "SELECT" && item.grantable === "NO"),
  );
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain('"grantable": "YES"');
  fresh = structuredClone(NEW_ROLE);
  (fresh.app_role?.routine_privileges as Array<{ execute: boolean }>)[0].execute = false;
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("app_role.routine_privileges");
  fresh = structuredClone(NEW_ROLE);
  fresh.app_role!.sequence_usage = false;
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF app_role.sequence_usage");
  fresh = structuredClone(NEW_ROLE);
  (fresh.app_role?.schema_privileges as { public: { create: boolean } }).public.create = true;
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF app_role.schema_privileges");
});

caseTest("role identity must match and stay unprivileged", () => {
  let fresh = withRole(NEW_LEDGER, NEW_COLS, { role: "other_app" });
  let result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("ROLE identity differs");
  for (const flag of ["superuser", "bypassrls", "createrole", "createdb", "replication"]) {
    const old = withRole(OLD_LEDGER, OLD_COLS, { attrs: attributes({ [flag]: true }) });
    fresh = withRole(NEW_LEDGER, NEW_COLS, { attrs: attributes({ [flag]: true }) });
    result = run(old, fresh);
    expect(result.rc, flag).toBe(1);
    expect(result.out, flag).toContain(`${flag}=True`);
  }
  fresh = withRole(NEW_LEDGER, NEW_COLS, { attrs: attributes({ inherit: false }) });
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("ROLE attributes differ");
  const old = withRole(OLD_LEDGER, OLD_COLS, {
    attrs: attributes({ member_of: ["pg_read_all_data"] }),
  });
  fresh = withRole(NEW_LEDGER, NEW_COLS, {
    attrs: attributes({ member_of: ["pg_read_all_data"] }),
  });
  result = run(old, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("is a member of");
  fresh = withRole(NEW_LEDGER, NEW_COLS, { attrs: attributes({ exists: false }) });
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("role absent");
});

caseTest("missing or unexpected metadata fails", () => {
  let fresh = structuredClone(NEW_ROLE);
  delete fresh.app_role?.attributes;
  let result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("metadata keys");
  fresh = structuredClone(NEW_ROLE);
  fresh.app_role!.note = "x";
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("missing/unexpected");
  for (const field of ["grantable", "grantor", "schema"]) {
    fresh = structuredClone(NEW_ROLE);
    delete (fresh.app_role?.column_privileges as Array<Record<string, unknown>>)[0][field];
    result = run(OLD_ROLE, fresh);
    expect(result.rc, field).toBe(1);
    expect(result.out, field).toContain("metadata keys differ");
  }
  fresh = structuredClone(NEW_ROLE);
  (fresh.app_role?.table_privileges as Array<Record<string, unknown>>)[0].extra = 1;
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("metadata keys differ");
  fresh = structuredClone(NEW_ROLE);
  (fresh.app_role?.table_privileges as Array<{ grantable: string }>)[1].grantable = "MAYBE";
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("are not NO/YES");
  fresh = structuredClone(NEW_ROLE);
  delete (fresh.app_role?.attributes as Record<string, unknown>).bypassrls;
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("attributes incomplete");
});

caseTest("ledger acl authority is never discarded", () => {
  const cases: Array<[string, string | null, string]> = [
    [
      "foreign grantee read",
      acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/${OWNER}`, `other=r/${OWNER}`]),
      "foreign grantee 'other'",
    ],
    [
      "another recipient write",
      acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/${OWNER}`, `other=w/${OWNER}`]),
      "foreign grantee 'other'",
    ],
    [
      "PUBLIC read",
      acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/${OWNER}`, `=r/${OWNER}`]),
      "grants PUBLIC",
    ],
    [
      "app role grant option",
      acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r*/${OWNER}`]),
      "expected exactly 'r'",
    ],
    [
      "app role write",
      acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=rw/${OWNER}`]),
      "expected exactly 'r'",
    ],
    [
      "app role granted by non-owner",
      acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/other_admin`]),
      "not the owner",
    ],
    [
      "two full entries",
      acl([`${OWNER}=arwdDxtm/${OWNER}`, `other=arwdDxtm/other`, `${APP}=r/${OWNER}`]),
      "owner entries",
    ],
    ["no app role entry", acl([`${OWNER}=arwdDxtm/${OWNER}`]), "no entry for the app role"],
    ["no acl", null, "has no acl"],
    ["quoted identifier", `{"we ird"=r/${OWNER}}`, "unparsable"],
  ];
  for (const [label, tableAcl, expected] of cases) {
    const old = structuredClone(OLD_ROLE);
    const fresh = structuredClone(NEW_ROLE);
    ledgerTable(old).acl = tableAcl;
    ledgerTable(fresh).acl = tableAcl;
    const { rc, out } = run(old, fresh);
    expect(rc, label).toBe(1);
    expect(out, label).toContain(expected);
  }
  let fresh = structuredClone(NEW_ROLE);
  ledgerTable(fresh).acl = acl([
    `${OWNER}=arwdDxtm/${OWNER}`,
    `${APP}=r/${OWNER}`,
    `other=r/${OWNER}`,
  ]);
  let result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("LEDGER TABLE acl differs");
  fresh = structuredClone(NEW_ROLE);
  ledgerTable(fresh).rls = false;
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("LEDGER TABLE rls differs");
  fresh = structuredClone(NEW_ROLE);
  (ledgerTable(fresh).columns as Column[])[1].acl = acl([`${APP}=r/${OWNER}`]);
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("new column lineage carries a column acl");
  fresh = structuredClone(NEW_ROLE);
  (ledgerTable(fresh).columns as Column[])[0].acl = acl([`${APP}=w/${OWNER}`]);
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("LEDGER TABLE column version acl differs");
  expect(result.out).toContain("new column version carries a column acl");
  for (const [label, columnAcl] of [
    ["foreign write", "{foreign_app=w/postgres}"],
    ["PUBLIC write", "{=w/postgres}"],
    ["app read-only column grant", acl([`${APP}=r/${OWNER}`])],
    ["foreign read", "{other=r/postgres}"],
  ] as const) {
    const old = structuredClone(OLD_ROLE);
    fresh = structuredClone(NEW_ROLE);
    (ledgerTable(old).columns as Column[])[0].acl = columnAcl;
    (ledgerTable(fresh).columns as Column[])[0].acl = columnAcl;
    result = run(old, fresh);
    expect(result.rc, label).toBe(1);
    expect(result.out, label).toContain("old column version carries a column acl");
    expect(result.out, label).toContain("new column version carries a column acl");
    expect(result.out, label).not.toContain("column version acl differs");
  }
  fresh = structuredClone(NEW_ROLE);
  ledgerTable(fresh).policies = [
    {
      name: "p",
      cmd: "*",
      permissive: true,
      roles: [APP],
      public: false,
      using: "true",
      check: null,
    },
  ];
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("LEDGER TABLE policies differs");
});

caseTest("product table, column type, and index or constraint definition differences fail", () => {
  const missing = structuredClone(NEW_LEDGER);
  missing.tables = missing.tables.filter((item) => item.name !== "users");
  let result = run(OLD_LEDGER, missing);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("MISSING table users");
  const typed = structuredClone(NEW_LEDGER);
  (typed.tables[1].columns as Column[])[0].type = "bigint";
  result = run(OLD_LEDGER, typed);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF table users.columns id.type");
  const indexed = structuredClone(OLD_LEDGER);
  const changed = structuredClone(NEW_LEDGER);
  const index = {
    name: "users_email_idx",
    def: "CREATE INDEX users_email_idx ON fvoci.users USING btree (email)",
    unique: false,
    primary: false,
    valid: true,
  };
  (indexed.tables[1].indexes as unknown[]).push(index);
  (changed.tables[1].indexes as unknown[]).push({
    ...index,
    def: "CREATE INDEX users_email_idx ON fvoci.users USING hash (email)",
  });
  result = run(indexed, changed);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF table users.indexes users_email_idx.def");
  const constrained = structuredClone(OLD_LEDGER);
  const constraintChanged = structuredClone(NEW_LEDGER);
  const constraint = {
    name: "users_email_unique",
    type: "u",
    def: "UNIQUE (email)",
    deferrable: false,
    deferred: false,
    validated: true,
  };
  (constrained.tables[1].constraints as unknown[]).push(constraint);
  (constraintChanged.tables[1].constraints as unknown[]).push({
    ...constraint,
    def: "UNIQUE (lower(email))",
  });
  result = run(constrained, constraintChanged);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF table users.constraints users_email_unique.def");
});

caseTest(
  "integer magnitude, decimal one, quoted text, and CRLF catalogs keep the same verdict",
  () => {
    const oldText = JSON.stringify(OLD_LEDGER).replace(
      '"extensions":["plpgsql"]',
      '"extensions":[9223372036854775807]',
    );
    const newText = JSON.stringify(NEW_LEDGER).replace(
      '"extensions":["plpgsql"]',
      '"extensions":[9223372036854775806]',
    );
    let result = runRaw(oldText, newText);
    expect(result.rc).toBe(1);
    expect(result.out).toContain("9223372036854775807");
    expect(result.out).toContain("9223372036854775806");
    const decimalOld = JSON.stringify(OLD_LEDGER).replace(
      '"extensions":["plpgsql"]',
      '"extensions":[1]',
    );
    const decimalNew = JSON.stringify(NEW_LEDGER).replace(
      '"extensions":["plpgsql"]',
      '"extensions":[1.0]',
    );
    result = runRaw(decimalOld, decimalNew);
    expect(result.rc).toBe(0);
    const quoted = structuredClone(NEW_LEDGER);
    quoted.seeds.instance_settings_meta = [{ id: 1, revision: 0, label: "o'h\n한글" }];
    result = run(OLD_LEDGER, quoted);
    expect(result.rc).toBe(1);
    expect(result.out).toContain("DIFF seed instance_settings_meta");
    const body = JSON.stringify(OLD_LEDGER);
    const crlf = `{\r\n${body.slice(1)}`;
    result = runRaw(crlf, crlf);
    expect(result.out).toContain("RESULT: FAIL");
  },
);

caseTest("duplicate object names fail closed before a report", () => {
  const cat = structuredClone(OLD_LEDGER);
  cat.tables.push(structuredClone(cat.tables[1]));
  const { ts, py } = invoke({ "a.json": JSON.stringify(cat) }, [
    "<DIR>/a.json",
    "<DIR>/a.json",
    "--report",
    "<DIR>/report.md",
  ]);
  expect(py.status).toBe(1);
  expect(ts).toEqual(py);
  expect(ts.stderr).toBe("duplicate name 'users'\n");
});

caseTest("documented usage and help match except the program name", () => {
  const cases = [
    [],
    ["only"],
    ["a", "b", "--report"],
    ["a", "b", "--nope"],
    ["-h"],
    ["--help"],
    ["-h", "extra"],
    ["a", "--nope"],
  ];
  for (const args of cases) {
    const { ts, py } = invoke({}, args);
    expect(ts.status, args.join(" ")).toBe(py.status);
    expect(normalizeProg(ts.stdout), args.join(" ")).toBe(normalizeProg(py.stdout));
    expect(normalizeProg(ts.stderr), args.join(" ")).toBe(normalizeProg(py.stderr));
    expect(ts.report, args.join(" ")).toBeNull();
  }
});

caseTest("missing file and malformed JSON fail closed with the same exit code", () => {
  const missing = invoke({}, [
    "<DIR>/missing-a.json",
    "<DIR>/missing-b.json",
    "--report",
    "<DIR>/missing.md",
  ]);
  for (const outcome of [missing.py, missing.ts]) {
    expect(outcome.status).toBe(1);
    expect(outcome.stdout).toBe("");
    expect(outcome.stderr).toContain("No such file or directory");
    expect(outcome.report).toBeNull();
  }
  expect(missing.ts.stderr).toBe(exceptionLine(missing.py.stderr));
  for (const body of ["", "{", '{"a":1,}', Buffer.from([0xff])]) {
    const { ts, py } = invoke({ "good.json": "{}", "bad.json": body }, [
      "<DIR>/good.json",
      "<DIR>/bad.json",
      "--report",
      "<DIR>/bad.md",
    ]);
    for (const outcome of [py, ts]) {
      expect(outcome.status, String(body)).toBe(1);
      expect(outcome.stdout, String(body)).toBe("");
      expect(outcome.stderr.length, String(body)).toBeGreaterThan(0);
      expect(outcome.report, String(body)).toBeNull();
    }
    // compare-catalogs.md, "Input and usage failures": the short diagnosis keeps the exception class.
    const pyClass = exceptionLine(py.stderr).split(":")[0];
    const tsClass = ts.stderr.split(":")[0];
    expect(pyClass.split(".").at(-1), String(body)).toBe(tsClass);
  }
});

caseTest("a ledger integer longer than 4300 digits fails closed", () => {
  const baseOld = JSON.stringify(OLD_LEDGER);
  const baseNew = JSON.stringify(NEW_LEDGER);
  const inject = (digits: string) =>
    baseOld.replace('"ledger":[{"version":1}', `"ledger":[{"version":${digits}}`);
  const accepted = runRaw(inject("8".repeat(4300)), baseNew);
  expect(accepted.rc).toBe(0);
  expect(accepted.out).toContain("RESULT: PASS");
  const { ts, py } = invoke({ "a.json": inject("8".repeat(4301)), "b.json": baseNew }, UNREPORTED);
  for (const outcome of [py, ts]) {
    expect(outcome.status).toBe(1);
    expect(outcome.stdout).toBe("");
    expect(outcome.stderr).toContain("Exceeds the limit (4300 digits)");
    expect(outcome.stderr).toContain("value has 4301 digits");
  }
});

caseTest("a BOM before a ledger acl is unparsable", () => {
  const old = structuredClone(OLD_LEDGER);
  const fresh = structuredClone(NEW_LEDGER);
  const bomAcl = `\uFEFF${LEDGER_ACL}`;
  ledgerTable(old).acl = bomAcl;
  ledgerTable(fresh).acl = bomAcl;
  const { rc, out } = run(old, fresh);
  expect(rc).toBe(1);
  expect(out).toContain("unparsable acl");
  expect(out).toContain("\\ufeff");
});

caseTest("list and dict object names fail closed", () => {
  const cases: Array<[string, (cat: Catalog) => void, string]> = [
    [
      "schema name",
      (cat) => {
        (cat.schemas[0] as { name: unknown }).name = ["fvoci"];
      },
      "list",
    ],
    [
      "table name",
      (cat) => {
        (cat.tables[1] as { name: unknown }).name = ["users"];
      },
      "list",
    ],
    [
      "function signature",
      (cat) => {
        cat.functions.push({ signature: ["f"] });
      },
      "list",
    ],
    [
      "schema dict name",
      (cat) => {
        (cat.schemas[0] as { name: unknown }).name = { name: "fvoci" };
      },
      "dict",
    ],
  ];
  for (const [label, mutate, kind] of cases) {
    const cat = structuredClone(OLD_LEDGER);
    mutate(cat);
    const { ts, py } = invoke({ "a.json": JSON.stringify(cat) }, ["<DIR>/a.json", "<DIR>/a.json"]);
    for (const outcome of [py, ts]) {
      expect(outcome.status, label).toBe(1);
      expect(outcome.stdout, label).toBe("");
      expect(outcome.stderr, label).toContain(`unhashable type: '${kind}'`);
    }
  }
});

test("every captured Python outcome is still exercised", () => {
  expect(Object.keys(ORACLE.cases).filter((key) => !usedCases.has(key))).toEqual([]);
});
