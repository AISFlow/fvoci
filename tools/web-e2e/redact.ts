// Same substitutions as the shell sed expressions that retain server logs.

const SERVER_ASSIGNMENT =
  /(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL|FVOCI_LIBSQL_URL|FVOCI_LIBSQL_AUTH_TOKEN|FVOCI_TEST_TURSO_[A-Z0-9_]*URL|FVOCI_TEST_TURSO_AUTH_TOKEN)=[^\s]+/g;

const STARTUP_ASSIGNMENT =
  /(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL|FVOCI_LIBSQL_URL|FVOCI_LIBSQL_AUTH_TOKEN|FVOCI_TEST_TURSO_[A-Z0-9_]*URL|FVOCI_TEST_TURSO_AUTH_TOKEN|MEILI[A-Z_]*KEY|PASSWORD[A-Z_]*|ENCRYPTION_KEYS)=[^\s]+/g;

function urls(text: string): string {
  return text
    .replace(/postgres(ql)?:\/\/\S+/g, "postgres://redacted")
    .replace(/libsql:\/\/\S+/g, "libsql://redacted")
    .replace(/https:\/\/\S*\.turso\.io\S*/g, "https://redacted");
}

export function redactServerLog(text: string): string {
  return urls(text).replace(SERVER_ASSIGNMENT, "$1=redacted");
}

export function redactStartupLog(text: string): string {
  return urls(text).replace(STARTUP_ASSIGNMENT, "$1=redacted");
}
