import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "bun:test";

const HERE = dirname(fileURLToPath(import.meta.url));
const TS = join(HERE, "compare-catalogs.ts");
const PY = join(HERE, "compare-catalogs.py");
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

function table(name: string, columns: string[], constraints: Array<[string, string]> = [], indexes: Array<[string, string]> = [], tableAcl: string | null = null) {
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
    constraints: constraints.map(([constraint, def]) => ({ name: constraint, type: "c", def, deferrable: false, deferred: false, validated: true })),
    indexes: indexes.map(([index, def]) => ({ name: index, def, unique: false, primary: false, valid: true })),
    triggers: [],
    policies: [],
  };
}

function catalog(ledgerColumns: string[], ledgerRows: Array<Record<string, unknown>>, usersDefault = "now()"): Catalog {
  const users = table("users", ["id", "email"], [], [], USERS_ACL);
  users.columns[1].default = usersDefault;
  return {
    server_version: "18.0",
    schemas: [{ name: "fvoci", acl: null }],
    tables: [table("schema_migrations", ledgerColumns, [["schema_migrations_pkey", "PRIMARY KEY (version)"]], [], LEDGER_ACL), users],
    sequences: [],
    functions: [],
    views: [],
    extensions: ["plpgsql"],
    seeds: { instance_settings_meta: [{ id: 1, revision: 0 }], instance_config: [{ id: 1 }], outbox_consumers: [], row_counts: { users: 0 } },
    ledger: ledgerRows,
  };
}

function tp(tableName: string, privilege: string, grantor = OWNER, grantable = "NO", schema = "fvoci") {
  return { schema, table: tableName, privilege, grantor, grantable };
}

function cp(tableName: string, column: string, privilege: string, grantor = OWNER, grantable = "NO", schema = "fvoci") {
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

function withRole(cat: Catalog, ledgerColumns: string[], options: {
  role?: string;
  grantor?: string;
  ledgerGrantable?: string;
  extraTable?: Array<ReturnType<typeof tp>>;
  extraColumn?: Array<ReturnType<typeof cp>>;
  dropColumn?: Array<[string, string, string]>;
  attrs?: ReturnType<typeof attributes>;
} = {}) {
  const next = structuredClone(cat);
  const grantor = options.grantor ?? OWNER;
  const ledgerGrantable = options.ledgerGrantable ?? "NO";
  let tablePrivileges = [tp("schema_migrations", "SELECT", grantor, ledgerGrantable), tp("users", "SELECT", grantor), tp("users", "INSERT", grantor)];
  let columnPrivileges = ledgerColumns.map((column) => cp("schema_migrations", column, "SELECT", grantor, ledgerGrantable));
  for (const column of ["id", "email"]) for (const privilege of ["SELECT", "INSERT"]) columnPrivileges.push(cp("users", column, privilege, grantor));
  tablePrivileges = tablePrivileges.concat(options.extraTable ?? []);
  columnPrivileges = columnPrivileges.concat(options.extraColumn ?? []);
  const dropped = new Set((options.dropColumn ?? []).map((item) => item.join("\0")));
  columnPrivileges = columnPrivileges.filter((item) => !dropped.has([item.table, item.column, item.privilege].join("\0")));
  next.app_role = {
    role: options.role ?? APP,
    attributes: options.attrs ?? attributes(),
    schema_privileges: { fvoci: { usage: true, create: false }, public: { usage: true, create: false } },
    table_privileges: tablePrivileges,
    column_privileges: columnPrivileges,
    routine_privileges: [{ routine: "fvoci.app_now()", execute: true }],
    sequence_usage: true,
    schema_usage: true,
  };
  return next;
}

function capture(command: string, args: string[]) {
  const proc = spawnSync(command, args, { encoding: "utf8" });
  return { status: proc.status, stdout: proc.stdout ?? "", stderr: proc.stderr ?? "" };
}

function runRaw(oldText: string, newText: string) {
  const dir = mkdtempSync(join(tmpdir(), "compare-catalogs-"));
  try {
    const oldPath = join(dir, "a.json");
    const newPath = join(dir, "b.json");
    const pyReport = join(dir, "py.md");
    const tsReport = join(dir, "ts.md");
    writeFileSync(oldPath, oldText);
    writeFileSync(newPath, newText);
    const py = capture("python3", [PY, oldPath, newPath, "--report", pyReport]);
    const ts = capture(process.execPath, [TS, oldPath, newPath, "--report", tsReport]);
    expect(py.status).not.toBeNull();
    expect(ts.status).not.toBeNull();
    expect(ts.status).toBe(py.status);
    expect(ts.stdout).toBe(py.stdout);
    expect(ts.stderr).toBe(py.stderr);
    expect(readFileSync(tsReport, "utf8")).toBe(readFileSync(pyReport, "utf8"));
    expect(ts.stdout).toBe(readFileSync(tsReport, "utf8"));
    return { rc: ts.status ?? -1, out: ts.stdout };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function run(old: unknown, fresh: unknown) {
  return runRaw(JSON.stringify(old), JSON.stringify(fresh));
}

function normalizeProg(text: string) {
  return text.replaceAll("compare-catalogs.py", "PROG").replaceAll("compare-catalogs.ts", "PROG");
}

const OLD_LEDGER = catalog(OLD_COLS, Array.from({ length: 55 }, (_, index) => ({ version: index + 1 })));
const NEW_LEDGER = catalog(NEW_COLS, Array.from({ length: 12 }, (_, index) => ({ version: index + 1, lineage: "fvoci-postgres-060", sql_sha256: (index + 1).toString(16).padStart(64, "0") })));
const OLD_ROLE = withRole(OLD_LEDGER, OLD_COLS);
const NEW_ROLE = withRole(NEW_LEDGER, NEW_COLS);

function ledgerTable(cat: Catalog) {
  const found = cat.tables.find((item) => item.name === "schema_migrations");
  if (!found) throw new Error("missing ledger");
  return found;
}

test("ledger table and rows are the only declared exception", () => {
  const { rc, out } = run(OLD_LEDGER, NEW_LEDGER);
  expect(rc).toBe(0);
  expect(out).toContain("RESULT: PASS");
  expect(out).toContain("LEDGER TABLE (declared exception");
  expect(out).toContain("LEDGER ROWS (declared exception");
  expect(out).not.toContain("DIFF table schema_migrations");
});

test("a product column default difference fails", () => {
  const fresh = structuredClone(NEW_LEDGER);
  (fresh.tables[1].columns as Column[])[1].default = "'x'::text";
  const { rc, out } = run(OLD_LEDGER, fresh);
  expect(rc).toBe(1);
  expect(out).toContain("DIFF table users");
  expect(out).toContain("RESULT: FAIL");
});

test("a missing product constraint or index fails", () => {
  const cases = [
    ["constraints", { name: "users_email_unique", type: "u", def: "UNIQUE (email)", deferrable: false, deferred: false, validated: true }],
    ["indexes", { name: "users_email_idx", def: "CREATE INDEX users_email_idx ON fvoci.users USING btree (email)", unique: false, primary: false, valid: true }],
  ] as const;
  for (const [key, item] of cases) {
    const old = structuredClone(OLD_LEDGER);
    (old.tables[1][key] as unknown[]).push(item);
    const { rc, out } = run(old, NEW_LEDGER);
    expect(rc).toBe(1);
    expect(out).toContain(`MISSING table users.${key} ${item.name}`);
  }
});

test("malformed new ledger is a failure not an exception", () => {
  const cases: Array<[string, (cat: Catalog) => void]> = [
    ["wrong lineage", (cat) => { cat.ledger[3] = { ...cat.ledger[3], lineage: "fvoci-postgres-999" }; }],
    ["missing digest", (cat) => { cat.ledger[5] = { version: 6, lineage: "fvoci-postgres-060" }; }],
    ["short digest", (cat) => { cat.ledger[1] = { ...cat.ledger[1], sql_sha256: "abc" }; }],
    ["gap in versions", (cat) => { cat.ledger.splice(4, 1); }],
    ["duplicate digest", (cat) => { cat.ledger[2] = { ...cat.ledger[2], sql_sha256: cat.ledger[1].sql_sha256 }; }],
    ["extra ledger column", (cat) => { (ledgerTable(cat).columns as Column[]).push({ num: 5, name: "note", type: "text", notnull: false, default: null, identity: "", generated: "", collation: "-", acl: null }); }],
    ["nullable digest column", (cat) => { (ledgerTable(cat).columns as Column[])[2].notnull = false; }],
    ["no rows", (cat) => { cat.ledger = []; }],
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

test("a missing ledger table is reported not silently excepted", () => {
  const fresh = structuredClone(NEW_LEDGER);
  fresh.tables = fresh.tables.filter((item) => item.name !== "schema_migrations");
  const { rc, out } = run(OLD_LEDGER, fresh);
  expect(rc).toBe(1);
  expect(out).toContain("MISSING ledger table schema_migrations");
});

test("seed and column order differences fail", () => {
  const seeded = structuredClone(NEW_LEDGER);
  seeded.seeds.instance_settings_meta = [{ id: 1, revision: 1 }];
  let result = run(OLD_LEDGER, seeded);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF seed instance_settings_meta");
  const reordered = structuredClone(NEW_LEDGER);
  (reordered.tables[1].columns as Column[]).reverse();
  (reordered.tables[1].columns as Column[]).forEach((column, index) => { column.num = index + 1; });
  result = run(OLD_LEDGER, reordered);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("COLUMN-ORDER table users");
});

test("role inclusive ledger select expansion passes", () => {
  const { rc, out } = run(OLD_ROLE, NEW_ROLE);
  expect(rc).toBe(0);
  expect(out).toContain("RESULT: PASS");
  const flagged = out.split("\n").filter((line) => line.startsWith("EXTRA") || line.startsWith("MISSING") || line.startsWith("DIFF")).join("\n");
  expect(flagged).not.toContain("app_role");
});

test("unauthorized or incomplete ledger privileges fail", () => {
  const cases: Record<string, Catalog> = {
    "new INSERT on ledger table": withRole(NEW_LEDGER, NEW_COLS, { extraTable: [tp("schema_migrations", "INSERT")] }),
    "new UPDATE on lineage column": withRole(NEW_LEDGER, NEW_COLS, { extraColumn: [cp("schema_migrations", "lineage", "UPDATE")] }),
    "new DELETE on ledger table": withRole(NEW_LEDGER, NEW_COLS, { extraTable: [tp("schema_migrations", "DELETE")] }),
    "new SELECT missing on sql_sha256": withRole(NEW_LEDGER, ["version", "lineage", "applied_at"]),
    "new SELECT on a column the ledger does not have": withRole(NEW_LEDGER, NEW_COLS.concat(["note"])),
    "new table SELECT missing": withRole(NEW_LEDGER, NEW_COLS),
    "new ledger SELECT grantable (table and columns)": withRole(NEW_LEDGER, NEW_COLS, { ledgerGrantable: "YES" }),
    "new ledger column SELECT grantable only": withRole(NEW_LEDGER, NEW_COLS, { extraColumn: [cp("schema_migrations", "version", "SELECT", OWNER, "YES")], dropColumn: [["schema_migrations", "version", "SELECT"]] }),
    "new ledger grant from a second grantor": withRole(NEW_LEDGER, NEW_COLS, { extraTable: [tp("schema_migrations", "SELECT", "other_admin")] }),
  };
  const role = cases["new table SELECT missing"].app_role;
  if (!role) throw new Error("missing role");
  role.table_privileges = (role.table_privileges as Array<{ table: string }>).filter((item) => item.table !== "schema_migrations");
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

test("non ledger grant differences still fail", () => {
  let fresh = withRole(NEW_LEDGER, NEW_COLS, { extraColumn: [cp("users", "email", "UPDATE")] });
  let result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("EXTRA app_role.column_privileges");
  expect(result.out).toContain('"privilege": "UPDATE"');
  fresh = withRole(NEW_LEDGER, NEW_COLS, { dropColumn: [["users", "id", "SELECT"]] });
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("MISSING app_role.column_privileges");
  fresh = withRole(NEW_LEDGER, NEW_COLS, { extraTable: [tp("users", "SELECT", OWNER, "NO", "public")] });
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("EXTRA app_role.table_privileges");
  expect(result.out).toContain('"schema": "public"');
  fresh = withRole(NEW_LEDGER, NEW_COLS, { extraTable: [tp("users", "SELECT", OWNER, "YES")] });
  const privileges = fresh.app_role?.table_privileges as Array<{ table: string; privilege: string; grantable: string }>;
  fresh.app_role!.table_privileges = privileges.filter((item) => !(item.table === "users" && item.privilege === "SELECT" && item.grantable === "NO"));
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

test("role identity must match and stay unprivileged", () => {
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
  const old = withRole(OLD_LEDGER, OLD_COLS, { attrs: attributes({ member_of: ["pg_read_all_data"] }) });
  fresh = withRole(NEW_LEDGER, NEW_COLS, { attrs: attributes({ member_of: ["pg_read_all_data"] }) });
  result = run(old, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("is a member of");
  fresh = withRole(NEW_LEDGER, NEW_COLS, { attrs: attributes({ exists: false }) });
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("role absent");
});

test("missing or unexpected metadata fails", () => {
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

test("ledger acl authority is never discarded", () => {
  const cases: Array<[string, string | null, string]> = [
    ["foreign grantee read", acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/${OWNER}`, `other=r/${OWNER}`]), "foreign grantee 'other'"],
    ["another recipient write", acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/${OWNER}`, `other=w/${OWNER}`]), "foreign grantee 'other'"],
    ["PUBLIC read", acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/${OWNER}`, `=r/${OWNER}`]), "grants PUBLIC"],
    ["app role grant option", acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r*/${OWNER}`]), "expected exactly 'r'"],
    ["app role write", acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=rw/${OWNER}`]), "expected exactly 'r'"],
    ["app role granted by non-owner", acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/other_admin`]), "not the owner"],
    ["two full entries", acl([`${OWNER}=arwdDxtm/${OWNER}`, `other=arwdDxtm/other`, `${APP}=r/${OWNER}`]), "owner entries"],
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
  ledgerTable(fresh).acl = acl([`${OWNER}=arwdDxtm/${OWNER}`, `${APP}=r/${OWNER}`, `other=r/${OWNER}`]);
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
  ledgerTable(fresh).policies = [{ name: "p", cmd: "*", permissive: true, roles: [APP], public: false, using: "true", check: null }];
  result = run(OLD_ROLE, fresh);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("LEDGER TABLE policies differs");
});

test("product table, column type, and index or constraint definition differences fail", () => {
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
  const index = { name: "users_email_idx", def: "CREATE INDEX users_email_idx ON fvoci.users USING btree (email)", unique: false, primary: false, valid: true };
  (indexed.tables[1].indexes as unknown[]).push(index);
  (changed.tables[1].indexes as unknown[]).push({ ...index, def: "CREATE INDEX users_email_idx ON fvoci.users USING hash (email)" });
  result = run(indexed, changed);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF table users.indexes users_email_idx.def");
  const constrained = structuredClone(OLD_LEDGER);
  const constraintChanged = structuredClone(NEW_LEDGER);
  const constraint = { name: "users_email_unique", type: "u", def: "UNIQUE (email)", deferrable: false, deferred: false, validated: true };
  (constrained.tables[1].constraints as unknown[]).push(constraint);
  (constraintChanged.tables[1].constraints as unknown[]).push({ ...constraint, def: "UNIQUE (lower(email))" });
  result = run(constrained, constraintChanged);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("DIFF table users.constraints users_email_unique.def");
});

test("integer magnitude, decimal one, quoted text, and CRLF catalogs keep the same verdict", () => {
  const oldText = JSON.stringify(OLD_LEDGER).replace('"extensions":["plpgsql"]', '"extensions":[9223372036854775807]');
  const newText = JSON.stringify(NEW_LEDGER).replace('"extensions":["plpgsql"]', '"extensions":[9223372036854775806]');
  let result = runRaw(oldText, newText);
  expect(result.rc).toBe(1);
  expect(result.out).toContain("9223372036854775807");
  expect(result.out).toContain("9223372036854775806");
  const decimalOld = JSON.stringify(OLD_LEDGER).replace('"extensions":["plpgsql"]', '"extensions":[1]');
  const decimalNew = JSON.stringify(NEW_LEDGER).replace('"extensions":["plpgsql"]', '"extensions":[1.0]');
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
});

test("duplicate object names fail closed before a report", () => {
  const dir = mkdtempSync(join(tmpdir(), "compare-catalogs-dup-"));
  try {
    const cat = structuredClone(OLD_LEDGER);
    cat.tables.push(structuredClone(cat.tables[1]));
    const path = join(dir, "a.json");
    const report = join(dir, "report.md");
    writeFileSync(path, JSON.stringify(cat));
    const py = capture("python3", [PY, path, path, "--report", report]);
    const ts = capture(process.execPath, [TS, path, path, "--report", report]);
    expect(py.status).toBe(1);
    expect(ts.status).toBe(py.status);
    expect(ts.stdout).toBe(py.stdout);
    expect(ts.stderr).toBe(py.stderr);
    expect(ts.stderr).toBe("duplicate name 'users'\n");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("documented usage and help match except the program name", () => {
  const cases = [[], ["only"], ["a", "b", "--report"], ["a", "b", "--nope"], ["-h"], ["--help"], ["-h", "extra"], ["a", "--nope"]];
  for (const args of cases) {
    const py = capture("python3", [PY, ...args]);
    const ts = capture(process.execPath, [TS, ...args]);
    expect(ts.status, args.join(" ")).toBe(py.status);
    expect(normalizeProg(ts.stdout), args.join(" ")).toBe(normalizeProg(py.stdout));
    expect(normalizeProg(ts.stderr), args.join(" ")).toBe(normalizeProg(py.stderr));
  }
});

test("missing file and malformed JSON fail closed with the same exit code", () => {
  const dir = mkdtempSync(join(tmpdir(), "compare-catalogs-bad-"));
  try {
    const missingPy = capture("python3", [PY, join(dir, "missing-a.json"), join(dir, "missing-b.json"), "--report", join(dir, "missing.md")]);
    const missingTs = capture(process.execPath, [TS, join(dir, "missing-a.json"), join(dir, "missing-b.json"), "--report", join(dir, "missing.md")]);
    expect(missingPy.status).toBe(1);
    expect(missingTs.status).toBe(1);
    expect(missingPy.stdout).toBe("");
    expect(missingTs.stdout).toBe("");
    expect(missingPy.stderr).toContain("No such file or directory");
    expect(missingTs.stderr).toContain("No such file or directory");
    for (const body of ["", "{", '{"a":1,}', Buffer.from([0xff])]) {
      const path = join(dir, "bad.json");
      writeFileSync(path, body);
      const good = join(dir, "good.json");
      writeFileSync(good, "{}");
      const report = join(dir, "bad.md");
      const py = capture("python3", [PY, good, path, "--report", report]);
      const ts = capture(process.execPath, [TS, good, path, "--report", report]);
      expect(py.status, body).toBe(1);
      expect(ts.status, body).toBe(1);
      expect(py.stdout, body).toBe("");
      expect(ts.stdout, body).toBe("");
      expect(py.stderr.length, body).toBeGreaterThan(0);
      expect(ts.stderr.length, body).toBeGreaterThan(0);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
