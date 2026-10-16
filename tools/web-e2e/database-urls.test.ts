import { describe, expect, test } from "bun:test";
import path from "node:path";
import {
  DatabaseUrlError,
  databaseUrls,
  groupDatabaseStatement,
  fixtureDatabaseUrl,
  fixtureQueryArgs,
  fixtureRows,
  quoteComponent,
} from "./database-urls.ts";

const SCRIPT = path.join(import.meta.dirname, "database-urls.ts");
const DB = "fvoci_e2e_0123456789abcdef";
const ROLE = `fvoci_app_${DB}`;
const PASSWORD = "00112233445566778899aabbccddeeff";
const SECRET = "admin-secret";

test("fixture rows preserve text microseconds, null columns, duplicates and multiple statement results", () => {
  const row = [Buffer.from("2026-10-16 01:02:03.123456+00"), null, Buffer.from("t")];
  expect(fixtureRows([[], [row, row], []])).toBe(
    "2026-10-16 01:02:03.123456+00||t\n2026-10-16 01:02:03.123456+00||t",
  );
  expect(fixtureRows([[null], [Buffer.from("x")], [null]])).toBe("\nx\n");
  expect(() => fixtureRows([[new Date()]])).toThrow(DatabaseUrlError);
});

test("fixture app URL is bound to the generated admin database and same service", () => {
  const env = {
    FVOCI_TEST_PG_CONTAINER: "fvoci-rust-test-pg-" + "a".repeat(32),
    FVOCI_E2E_ADMIN_DATABASE_URL: `postgres://postgres:${SECRET}@fvoci-pg:5432/${DB}`,
    DATABASE_APP_URL: `postgres://${ROLE}:${PASSWORD}@fvoci-pg:5432/${DB}`,
  };
  expect(fixtureDatabaseUrl("app", env)).toBe(env.DATABASE_APP_URL);
  for (const change of [
    { DATABASE_APP_URL: env.DATABASE_APP_URL.replace(ROLE, "postgres") },
    { DATABASE_APP_URL: env.DATABASE_APP_URL.replace("fvoci-pg", "foreign") },
    { FVOCI_E2E_ADMIN_DATABASE_URL: env.FVOCI_E2E_ADMIN_DATABASE_URL.replace(DB, "postgres") },
  ])
    expect(() => fixtureDatabaseUrl("app", { ...env, ...change })).toThrow(DatabaseUrlError);
});

describe("explicit owned archive destinations", () => {
  const owned = "fvoci_e2e_abcdef0123456789";
  const role = `fvoci_app_${owned}`;
  const env = {
    FVOCI_TEST_PG_CONTAINER: "fvoci-rust-test-pg-" + "a".repeat(32),
    FVOCI_E2E_ADMIN_DATABASE_URL: `postgres://postgres:${SECRET}@fvoci-pg:5432/${DB}`,
    DATABASE_APP_URL: `postgres://${role}:${PASSWORD}@fvoci-pg:5432/${owned}`,
    DB_NAME: owned,
    ROLE_NAME: role,
  };
  test("binds the separate generated pair to the group service and fixed maintenance database", () => {
    expect(fixtureDatabaseUrl("app", env, true)).toBe(env.DATABASE_APP_URL);
    expect(new URL(fixtureDatabaseUrl("admin", env, true)).pathname).toBe(`/${owned}`);
    expect(new URL(fixtureDatabaseUrl("maintenance", env, true)).pathname).toBe("/postgres");
    expect(new URL(fixtureDatabaseUrl("admin", env, true)).username).toBe("postgres");
    expect(() => fixtureDatabaseUrl("app", env)).toThrow(DatabaseUrlError);
    expect(fixtureDatabaseUrl("admin", env)).toBe(env.FVOCI_E2E_ADMIN_DATABASE_URL);
    expect(() => fixtureDatabaseUrl("maintenance", env)).toThrow(DatabaseUrlError);
    expect(fixtureQueryArgs("app", false, true)).toEqual([
      "--no-env-file",
      SCRIPT,
      "fixture-query",
      "app",
      "owned",
    ]);
  });
  test("refuses foreign pairs, service identities and app destinations without disclosing inputs", () => {
    for (const mode of ["app", "admin", "maintenance"])
      for (const change of [
        { DB_NAME: "postgres" },
        { DB_NAME: owned + '"; DROP DATABASE postgres;--' },
        { ROLE_NAME: "postgres" },
        { ROLE_NAME: ROLE },
        { FVOCI_TEST_PG_CONTAINER: "foreign" },
        { FVOCI_E2E_ADMIN_DATABASE_URL: env.FVOCI_E2E_ADMIN_DATABASE_URL.replace(DB, "postgres") },
        { FVOCI_E2E_ADMIN_DATABASE_URL: "PRIVATE_invalid_connection" },
      ]) {
        expect(() => fixtureDatabaseUrl(mode, { ...env, ...change }, true)).toThrow(
          "owned PostgreSQL fixture role and database required",
        );
      }
    for (const url of [
      env.DATABASE_APP_URL.replace(role, "postgres"),
      env.DATABASE_APP_URL.replace("fvoci-pg", "foreign"),
      env.DATABASE_APP_URL.replace(":5432/", ":5433/"),
      env.DATABASE_APP_URL.replace(`/${owned}`, `/${DB}`),
      env.DATABASE_APP_URL.replace("postgres:", "https:"),
      "PRIVATE_invalid_connection",
    ])
      expect(() => fixtureDatabaseUrl("app", { ...env, DATABASE_APP_URL: url }, true)).toThrow(
        "owned PostgreSQL fixture role and database required",
      );
    for (const name of ["DB_NAME", "ROLE_NAME"])
      expect(() =>
        fixtureDatabaseUrl(
          "maintenance",
          Object.fromEntries(Object.entries(env).filter(([key]) => key !== name)),
          true,
        ),
      ).toThrow(DatabaseUrlError);
  });
});

describe("owned group database operations", () => {
  const env = { DB_NAME: DB, ROLE_NAME: ROLE, ROLE_PASSWORD: PASSWORD };
  test("the app role is explicitly restricted and retirement is limited to the same generated pair", () => {
    expect(groupDatabaseStatement("create-role", env)).toContain("NOSUPERUSER NOBYPASSRLS");
    expect(groupDatabaseStatement("drop-role", env)).toBe(`DROP ROLE IF EXISTS "${ROLE}"`);
    expect(groupDatabaseStatement("drop-database", env)).toBe(
      `DROP DATABASE IF EXISTS "${DB}" WITH (FORCE)`,
    );
  });
  for (const bad of [
    { DB_NAME: "postgres" },
    { DB_NAME: DB + '"; DROP DATABASE postgres;--' },
    { ROLE_NAME: "postgres" },
    { ROLE_PASSWORD: "' SUPERUSER --" },
  ])
    test("refuses foreign or injectable group inputs " + Object.keys(bad).join(), () => {
      expect(() => groupDatabaseStatement("create-role", { ...env, ...bad })).toThrow(
        DatabaseUrlError,
      );
    });
  test("refuses arbitrary SQL commands", () => {
    expect(() => groupDatabaseStatement("DROP DATABASE postgres", env)).toThrow(DatabaseUrlError);
  });
});

describe("databaseUrls", () => {
  test("replaces the admin path and builds the app role URL", () => {
    expect(
      databaseUrls(`postgres://postgres:${SECRET}@127.0.0.1:54321/postgres`, DB, ROLE, PASSWORD),
    ).toEqual({
      admin: `postgres://postgres:${SECRET}@127.0.0.1:54321/${DB}`,
      app: `postgres://${ROLE}:${PASSWORD}@127.0.0.1:54321/${DB}`,
    });
  });

  test("keeps the admin authority, query and fragment verbatim", () => {
    const { admin, app } = databaseUrls(
      "POSTGRESQL://a%40b:p%2Fw;x@Db.Example:5/postgres;v?sslmode=require#frag",
      DB,
      ROLE,
      PASSWORD,
    );
    expect(admin).toBe(`postgresql://a%40b:p%2Fw;x@Db.Example:5/${DB}?sslmode=require#frag`);
    expect(app).toBe(`postgres://${ROLE}:${PASSWORD}@db.example:5/${DB}`);
  });

  test("drops an empty query or fragment and defaults host and port", () => {
    expect(databaseUrls("postgres:///postgres?#", DB, ROLE, PASSWORD)).toEqual({
      admin: `postgres:///${DB}`,
      app: `postgres://${ROLE}:${PASSWORD}@127.0.0.1:5432/${DB}`,
    });
    expect(databaseUrls("postgres://u@h:/x", DB, ROLE, PASSWORD).app).toBe(
      `postgres://${ROLE}:${PASSWORD}@h:5432/${DB}`,
    );
  });

  test("keeps the brackets of an IPv6 host", () => {
    expect(databaseUrls("postgres://u:p@[::1]:5433/postgres", DB, ROLE, PASSWORD).app).toBe(
      `postgres://${ROLE}:${PASSWORD}@[::1]:5433/${DB}`,
    );
  });

  test("percent-encodes the role and password like quote(safe='')", () => {
    expect(quoteComponent("a-b_c.d~e!*'()@:/ %")).toBe("a-b_c.d~e%21%2A%27%28%29%40%3A%2F%20%25");
    expect(databaseUrls("postgres://h/x", DB, "r ole", "p@ss!").app).toBe(
      `postgres://r%20ole:p%40ss%21@h:5432/${DB}`,
    );
  });

  test.each([
    ["", "printable ASCII"],
    ["postgres://h/x\n", "printable ASCII"],
    [" postgres://h/x", "printable ASCII"],
    ["postgres://h\t/x", "printable ASCII"],
    ["postgres://hé/x", "printable ASCII"],
    ["host=h dbname=x", "printable ASCII"],
    ["mysql://h/x", "postgres:// or postgresql://"],
    ["postgres:/h/x", "postgres:// or postgresql://"],
    ["/postgres", "postgres:// or postgresql://"],
    ["postgres://u:p@h:99999/x", "not a valid URL"],
    ["postgres://u:p@h:port/x", "not a valid URL"],
    ["postgres://u:p@[::1/x", "not a valid URL"],
  ])("refuses admin URL %p", (url, reason) => {
    expect(() => databaseUrls(url, DB, ROLE, PASSWORD)).toThrow(reason);
  });

  test("refuses a database name that would need encoding, and empty role inputs", () => {
    for (const name of ["", "a/b", "a?b", "a b", "ä"]) {
      expect(() => databaseUrls("postgres://h/x", name, ROLE, PASSWORD)).toThrow(DatabaseUrlError);
    }
    expect(() => databaseUrls("postgres://h/x", DB, "", PASSWORD)).toThrow("non-empty");
    expect(() => databaseUrls("postgres://h/x", DB, ROLE, "")).toThrow("non-empty");
  });
});

function run(env: Record<string, string>, args: string[] = []) {
  const result = Bun.spawnSync([process.execPath, SCRIPT, ...args], {
    env: { PATH: process.env.PATH ?? "", ...env },
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    status: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}

describe("database-urls.ts CLI", () => {
  const env = {
    TEST_DATABASE_URL: `postgres://postgres:${SECRET}@127.0.0.1:5432/postgres`,
    DB_NAME: DB,
    ROLE_NAME: ROLE,
    ROLE_PASSWORD: PASSWORD,
  };

  test("prints the admin URL, then the app URL", () => {
    expect(run(env)).toEqual({
      status: 0,
      stdout: `postgres://postgres:${SECRET}@127.0.0.1:5432/${DB}\npostgres://${ROLE}:${PASSWORD}@127.0.0.1:5432/${DB}\n`,
      stderr: "",
    });
  });

  test.each(["TEST_DATABASE_URL", "DB_NAME", "ROLE_NAME", "ROLE_PASSWORD"])(
    "fails without %s",
    (name) => {
      const partial = Object.fromEntries(Object.entries(env).filter(([key]) => key !== name));
      expect(run(partial)).toEqual({
        status: 1,
        stdout: "",
        stderr: `database-urls: ${name} is not set\n`,
      });
    },
  );

  test("an invalid URL prints nothing on stdout and never echoes the URL", () => {
    const result = run({
      ...env,
      TEST_DATABASE_URL: `postgres://postgres:${SECRET}@h:99999/postgres`,
    });
    expect(result.status).toBe(1);
    expect(result.stdout).toBe("");
    expect(result.stderr).not.toContain(SECRET);
    expect(result.stderr).toBe(
      "database-urls: TEST_DATABASE_URL is not a valid URL (host or port)\n",
    );
  });

  test("refuses arguments", () => {
    const result = run(env, [PASSWORD]);
    expect(result.status).toBe(2);
    expect(result.stdout).toBe("");
  });
});
