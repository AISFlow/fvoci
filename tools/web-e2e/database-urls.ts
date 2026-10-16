// The two connection URLs of a web e2e group's own database, derived from the
// admin TEST_DATABASE_URL: the admin URL with its database path replaced, and
// the group's app role on the same host and port. Inputs come from the
// environment (TEST_DATABASE_URL, DB_NAME, ROLE_NAME, ROLE_PASSWORD) so the
// role password never appears in a process argv; stdout is the admin URL and
// then the app URL, one per line, written only when both are valid. Errors
// never echo an input value: the admin URL carries the superuser password.

import { SQL } from "bun";

export class DatabaseUrlError extends Error {}

// A postgres URL with an authority: scheme, authority, path, then query and
// fragment, split as urllib.parse.urlsplit does.
const POSTGRES_URL = /^(postgres(?:ql)?):\/\/([^/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/i;
// Printable ASCII only: control characters, spaces and non-ASCII are refused,
// never stripped or re-encoded.
const PRINTABLE_ASCII = /^[\x21-\x7e]+$/;
const DATABASE_NAME = /^[A-Za-z0-9_]+$/;

/** `urllib.parse.quote(text, safe="")`: everything but RFC 3986 unreserved. */
export function quoteComponent(text: string): string {
  return encodeURIComponent(text).replace(
    /[!'()*]/g,
    (char) => `%${char.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

export type DatabaseUrls = { admin: string; app: string };

export function databaseUrls(
  adminUrl: string,
  dbName: string,
  role: string,
  rolePassword: string,
): DatabaseUrls {
  if (!PRINTABLE_ASCII.test(adminUrl)) {
    throw new DatabaseUrlError("TEST_DATABASE_URL must be printable ASCII without spaces");
  }
  const parts = POSTGRES_URL.exec(adminUrl);
  if (parts === null) {
    throw new DatabaseUrlError("TEST_DATABASE_URL must be a postgres:// or postgresql:// URL");
  }
  let parsed: URL;
  try {
    parsed = new URL(adminUrl);
  } catch {
    throw new DatabaseUrlError("TEST_DATABASE_URL is not a valid URL (host or port)");
  }
  if (!DATABASE_NAME.test(dbName)) {
    throw new DatabaseUrlError("DB_NAME must be ASCII letters, digits and underscores");
  }
  if (role === "" || rolePassword === "") {
    throw new DatabaseUrlError("ROLE_NAME and ROLE_PASSWORD must be non-empty");
  }
  const [, scheme = "", authority = "", , query = "", fragment = ""] = parts;
  // The authority (and so the admin credentials) is kept byte for byte; an
  // empty "?" or "#" is dropped, as urlunsplit does.
  const admin =
    `${scheme.toLowerCase()}://${authority}/${dbName}` +
    (query.length > 1 ? query : "") +
    (fragment.length > 1 ? fragment : "");
  // An IPv6 host keeps its brackets; no host means the local default.
  const host = parsed.hostname.toLowerCase() || "127.0.0.1";
  const port = parsed.port || "5432";
  const app = `postgres://${quoteComponent(role)}:${quoteComponent(rolePassword)}@${host}:${port}/${dbName}`;
  return { admin, app };
}

function required(env: NodeJS.ProcessEnv, name: string): string {
  const value = env[name];
  if (value === undefined) throw new DatabaseUrlError(`${name} is not set`);
  return value;
}

/** Native Bun child argv; SQL stays on stdin and credentials stay in env. */
type FixtureQueryRole = "admin" | "app" | "maintenance";

export function fixtureQueryArgs(
  role: FixtureQueryRole,
  commandTag = false,
  owned = false,
): string[] {
  return [
    "--no-env-file",
    import.meta.path,
    "fixture-query",
    role,
    ...(owned ? ["owned"] : []),
    ...(commandTag ? ["command-tag"] : []),
  ];
}

export function fixtureDatabaseUrl(role: string, env: NodeJS.ProcessEnv, owned = false): string {
  try {
    const admin = new URL(required(env, "FVOCI_E2E_ADMIN_DATABASE_URL"));
    const container = required(env, "FVOCI_TEST_PG_CONTAINER");
    if (
      !/^fvoci-rust-test-pg-[a-f0-9]{32}$/.test(container) ||
      !["postgres:", "postgresql:"].includes(admin.protocol) ||
      admin.username !== "postgres" ||
      !/^\/fvoci_e2e_[a-f0-9]{16}$/.test(admin.pathname)
    )
      throw new Error();
    if (owned) {
      // Archive installations own another generated pair on this same fixture
      // service. Maintenance never accepts an arbitrary database destination.
      const database = required(env, "DB_NAME");
      if (
        !/^fvoci_e2e_[a-f0-9]{16}$/.test(database) ||
        required(env, "ROLE_NAME") !== `fvoci_app_${database}`
      )
        throw new Error();
      admin.pathname = role === "maintenance" ? "/postgres" : `/${database}`;
    }
    if (role === "admin" || (role === "maintenance" && owned)) return admin.toString();
    const app = new URL(required(env, "DATABASE_APP_URL"));
    if (
      role !== "app" ||
      !["postgres:", "postgresql:"].includes(app.protocol) ||
      app.username !== "fvoci_app_" + admin.pathname.slice(1) ||
      app.pathname !== admin.pathname ||
      app.hostname !== admin.hostname ||
      app.port !== admin.port
    )
      throw new Error();
    return app.toString();
  } catch {
    throw new DatabaseUrlError("owned PostgreSQL fixture role and database required");
  }
}

export function fixtureRows(value: unknown): string {
  if (!Array.isArray(value)) throw new DatabaseUrlError("invalid PostgreSQL fixture result");
  const lines: string[] = [];
  for (const row of value as unknown[]) {
    if (!Array.isArray(row)) throw new DatabaseUrlError("invalid PostgreSQL fixture row");
    if (row.length === 0) continue;
    if (row.every((field: unknown) => field === null || Buffer.isBuffer(field)))
      lines.push(
        row.map((field: unknown) => (field === null ? "" : (field as Buffer).toString())).join("|"),
      );
    else lines.push(fixtureRows(row));
  }
  return lines.join("\n");
}

async function fixtureQuery(role: string, commandTag: boolean, owned: boolean): Promise<void> {
  const connection = new SQL(fixtureDatabaseUrl(role, process.env, owned), {
    max: 1,
    connectionTimeout: 10,
  });
  try {
    const sql = await Bun.stdin.text();
    const rows: unknown = await connection.unsafe(sql).simple().raw();
    if (commandTag) {
      if (
        !Array.isArray(rows) ||
        !("command" in rows) ||
        rows.command !== "UPDATE" ||
        !("count" in rows) ||
        typeof rows.count !== "number" ||
        !Number.isSafeInteger(rows.count)
      )
        throw new DatabaseUrlError("invalid PostgreSQL command result");
      process.stdout.write(`UPDATE ${String(rows.count)}\n`);
    } else process.stdout.write(fixtureRows(rows));
  } catch (error) {
    if (error instanceof DatabaseUrlError) throw error;
    throw new DatabaseUrlError("isolated PostgreSQL fixture query failed");
  } finally {
    await connection.close();
  }
}

// The four operations formerly performed with docker exec psql by the group
// runtime. Keeping them here lets the same runtime use the owned PostgreSQL
// service directly, without a Docker socket or CLI inside the browser image.
export function groupDatabaseStatement(action: string, env: NodeJS.ProcessEnv): string {
  const database = required(env, "DB_NAME");
  const role = required(env, "ROLE_NAME");
  if (!/^fvoci_e2e_[a-f0-9]{16}$/.test(database) || role !== `fvoci_app_${database}`) {
    throw new DatabaseUrlError("only the group's generated database and role are allowed");
  }
  switch (action) {
    case "create-database":
      return `CREATE DATABASE "${database}"`;
    case "drop-database":
      return `DROP DATABASE IF EXISTS "${database}" WITH (FORCE)`;
    case "drop-role":
      return `DROP ROLE IF EXISTS "${role}"`;
    case "create-role": {
      const password = required(env, "ROLE_PASSWORD");
      if (!/^[a-f0-9]{32}$/.test(password)) {
        throw new DatabaseUrlError("the group's generated role password is required");
      }
      return `CREATE ROLE "${role}" LOGIN PASSWORD '${password}' NOSUPERUSER NOBYPASSRLS`;
    }
    default:
      throw new DatabaseUrlError("unknown group database operation");
  }
}

async function groupDatabase(action: string): Promise<void> {
  const statement = groupDatabaseStatement(action, process.env);
  const url = required(process.env, "TEST_DATABASE_URL");
  databaseUrls(url, required(process.env, "DB_NAME"), required(process.env, "ROLE_NAME"), "check");
  const connection = new SQL(url, { max: 1, connectionTimeout: 10 });
  try {
    await connection.unsafe(statement).simple();
    if (action === "create-role") {
      const urls = databaseUrls(
        url,
        required(process.env, "DB_NAME"),
        required(process.env, "ROLE_NAME"),
        required(process.env, "ROLE_PASSWORD"),
      );
      const app = new SQL(urls.app, { max: 1, connectionTimeout: 10 });
      try {
        const rows: unknown = await app`SELECT current_user AS role, current_database() AS database,
          r.rolsuper, r.rolbypassrls, d.datdba = r.oid AS owns_database
          FROM pg_roles r JOIN pg_database d ON d.datname = current_database()
          WHERE r.rolname = current_user`;
        if (
          !Array.isArray(rows) ||
          rows.length !== 1 ||
          typeof rows[0] !== "object" ||
          rows[0] === null ||
          Array.isArray(rows[0])
        )
          throw new DatabaseUrlError("group app-role identity result is invalid");
        const facts = rows[0] as Record<string, unknown>;
        if (
          facts.role !== process.env.ROLE_NAME ||
          facts.database !== process.env.DB_NAME ||
          facts.rolsuper !== false ||
          facts.rolbypassrls !== false ||
          facts.owns_database !== false
        )
          throw new DatabaseUrlError("group app-role identity or privilege mismatch");
        process.stdout.write(
          "web-e2e app-role connection: superuser=false bypassrls=false database-owner=false\n",
        );
      } finally {
        await app.close();
      }
    }
    if (action === "drop-database" || action === "drop-role") {
      const rows: unknown =
        action === "drop-database"
          ? await connection`SELECT datname FROM pg_database WHERE datname = ${process.env.DB_NAME}`
          : await connection`SELECT rolname FROM pg_roles WHERE rolname = ${process.env.ROLE_NAME}`;
      if (!Array.isArray(rows) || rows.length !== 0)
        throw new DatabaseUrlError("group database resource was not retired");
      process.stdout.write(`web-e2e ${action} verified absent\n`);
    }
  } catch (error) {
    if (error instanceof DatabaseUrlError) throw error;
    // Driver errors may include the connection or SQL (including the password).
    throw new DatabaseUrlError("group database operation failed");
  } finally {
    await connection.close();
  }
}

if (import.meta.main) {
  if (process.argv[2] === "fixture-query") {
    try {
      const owned = process.argv[4] === "owned";
      const option = owned ? 5 : 4;
      if (
        !["admin", "app", "maintenance"].includes(process.argv[3] ?? "") ||
        process.argv.length > option + 1 ||
        (process.argv[option] !== undefined && process.argv[option] !== "command-tag")
      )
        throw new DatabaseUrlError("invalid fixture query arguments");
      await fixtureQuery(process.argv[3] ?? "", process.argv[option] === "command-tag", owned);
      process.exit(0);
    } catch {
      process.stderr.write("database-urls: isolated PostgreSQL fixture query failed\n");
      process.exit(1);
    }
  }
  if (
    process.argv.length > 3 ||
    (process.argv[2] !== undefined &&
      !["create-database", "create-role", "drop-database", "drop-role"].includes(process.argv[2]))
  ) {
    process.stderr.write(
      "usage: TEST_DATABASE_URL=… DB_NAME=… ROLE_NAME=… ROLE_PASSWORD=… database-urls.ts\n",
    );
    process.exit(2);
  }
  try {
    if (process.argv[2]) {
      await groupDatabase(process.argv[2]);
      process.exit(0);
    }
    const env = process.env;
    const urls = databaseUrls(
      required(env, "TEST_DATABASE_URL"),
      required(env, "DB_NAME"),
      required(env, "ROLE_NAME"),
      required(env, "ROLE_PASSWORD"),
    );
    process.stdout.write(`${urls.admin}\n${urls.app}\n`);
  } catch (error) {
    if (!(error instanceof DatabaseUrlError)) throw error;
    process.stderr.write(`database-urls: ${error.message}\n`);
    process.exit(1);
  }
}
