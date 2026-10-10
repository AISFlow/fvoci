// The two connection URLs of a web e2e group's own database, derived from the
// admin TEST_DATABASE_URL: the admin URL with its database path replaced, and
// the group's app role on the same host and port. Inputs come from the
// environment (TEST_DATABASE_URL, DB_NAME, ROLE_NAME, ROLE_PASSWORD) so the
// role password never appears in a process argv; stdout is the admin URL and
// then the app URL, one per line, written only when both are valid. Errors
// never echo an input value: the admin URL carries the superuser password.

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

if (import.meta.main) {
  if (process.argv.length > 2) {
    process.stderr.write(
      "usage: TEST_DATABASE_URL=… DB_NAME=… ROLE_NAME=… ROLE_PASSWORD=… database-urls.ts\n",
    );
    process.exit(2);
  }
  try {
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
