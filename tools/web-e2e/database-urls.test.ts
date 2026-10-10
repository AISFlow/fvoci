import { describe, expect, test } from "bun:test";
import path from "node:path";
import { DatabaseUrlError, databaseUrls, quoteComponent } from "./database-urls.ts";

const SCRIPT = path.join(import.meta.dirname, "database-urls.ts");
const DB = "fvoci_e2e_0123456789abcdef";
const ROLE = `fvoci_app_${DB}`;
const PASSWORD = "00112233445566778899aabbccddeeff";
const SECRET = "admin-secret";

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
