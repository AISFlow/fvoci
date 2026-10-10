import { describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

function layout() {
  const root = mkdtempSync(join(tmpdir(), "fvoci-startup-"));
  const bin = mkdtempSync(join(tmpdir(), "fvoci-startup-bin-"));
  const run = join(root, "run");
  mkdirSync(join(root, "scripts"), { recursive: true });
  mkdirSync(join(root, "target/debug"), { recursive: true });
  mkdirSync(join(run, "static"), { recursive: true });
  writeFileSync(
    join(root, "scripts/start-test-postgres.sh"),
    '#!/usr/bin/env bash\nexport TEST_DATABASE_URL=postgres://postgres:fixture-secret@127.0.0.1:5432/postgres\nexport FVOCI_TEST_PG_CONTAINER=fvoci-fixture-pg\n"$@"\n',
  );
  writeFileSync(join(root, "scripts/start-test-meili.sh"), '#!/usr/bin/env bash\n"$@"\n');
  writeFileSync(join(root, "target/debug/fvoci-migrate"), "#!/usr/bin/env bash\nexit 0\n");
  writeFileSync(
    join(root, "target/debug/fvoci-server"),
    `#!/usr/bin/env bash
echo "fvoci-server listening on http://127.0.0.1:9"
echo "probe DATABASE_APP_URL=\${DATABASE_APP_URL:-} admin \${FVOCI_E2E_ADMIN_DATABASE_URL-unset}"
echo "probe FVOCI_LIBSQL_URL=libsql://libsql-url-secret.example.test FVOCI_LIBSQL_AUTH_TOKEN=libsql-auth-secret FVOCI_TEST_TURSO_DATABASE_URL=https://turso-url-secret.example.test FVOCI_TEST_TURSO_AUTH_TOKEN=turso-auth-secret"
echo "bare libsql://libsql-url-secret.example.test"
echo "bare https://libsql-url-secret.aws-ap-northeast-1.turso.io"
if [[ "\${FVOCI_FIXTURE_SETUP:-ok}" == earlydeath ]]; then
  echo "fixture server exited before setup"
  exit 42
fi
exec /bin/sleep 600
`,
  );
  writeFileSync(
    join(bin, "docker"),
    `#!/usr/bin/env bash
if [[ "\${1:-}" == exec && "\${2:-}" == -i && "\${4:-}" == psql ]]; then exit 0; fi
echo "unexpected docker invocation: $*" >&2
exit 1
`,
  );
  writeFileSync(
    join(bin, "curl"),
    `#!/usr/bin/env bash
if [[ "$*" == "-fsS http://127.0.0.1:9/api/v1/setup" ]]; then
  printf 'probe %s\\n' "$*" >> "$FVOCI_FIXTURE_MARKS"
  exit 22
fi
echo "unexpected curl invocation: $*" >&2
exit 1
`,
  );
  writeFileSync(join(bin, "sleep"), "#!/usr/bin/env bash\nexit 0\n");
  writeFileSync(
    join(bin, "seq"),
    `#!/usr/bin/env bash
if [[ "$*" == "1 120" ]]; then
  for _ in $(/usr/bin/seq 1 100); do
    if [[ -f "$SERVER_LOG" ]] && grep -q 'fvoci-server listening on ' "$SERVER_LOG"; then
      exec /usr/bin/seq 1 120
    fi
    /bin/sleep 0.01
  done
  echo "fixture server did not publish its listening line" >&2
  exit 1
fi
exec /usr/bin/seq "$@"
`,
  );
  writeFileSync(
    join(bin, "bun"),
    `#!/usr/bin/env bash
if [[ "\${1:-}" == --bun && "\${4:-}" == playwright && "\${5:-}" == test ]]; then
  echo launch >> "$FVOCI_FIXTURE_MARKS"
  exit 0
fi
exec "$FVOCI_REAL_BUN" "$@"
`,
  );
  for (const path of [
    "scripts/start-test-postgres.sh",
    "scripts/start-test-meili.sh",
    "target/debug/fvoci-migrate",
    "target/debug/fvoci-server",
    "docker",
    "curl",
    "sleep",
    "seq",
    "bun",
  ].map((name) => (name.includes("/") ? join(root, name) : join(bin, name)))) {
    chmodSync(path, 0o755);
  }
  return { root, bin, run };
}

async function runInner(mode: string) {
  const fx = layout();
  const marks = join(fx.root, "marks");
  const child = Bun.spawn(["bun", join(import.meta.dir, "inner.ts")], {
    env: {
      ...process.env,
      PATH: `${fx.bin}:${process.env.PATH}`,
      ROOT: fx.root,
      RUN_DIR: fx.run,
      SERVER_LOG: join(fx.run, "server.log"),
      PEPPER: '{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}',
      CARGO_TARGET_DIR: join(fx.root, "target"),
      FVOCI_STATIC_DIR: join(fx.run, "static"),
      TEST_DATABASE_URL: "postgres://postgres:fixture-secret@127.0.0.1:5432/postgres",
      FVOCI_TEST_PG_CONTAINER: "fvoci-fixture-pg",
      FVOCI_FIXTURE_SETUP: mode,
      FVOCI_FIXTURE_MARKS: marks,
      FVOCI_REAL_BUN: process.execPath,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  const stderr = await new Response(child.stderr).text();
  const status = await child.exited;
  const marksText = (() => {
    try {
      return readFileSync(marks, "utf8");
    } catch {
      return "";
    }
  })();
  const launches = marksText.split("\n").filter((line) => line === "launch").length;
  const probes = marksText.split("\n").filter((line) => line.startsWith("probe")).length;
  rmSync(fx.root, { recursive: true, force: true });
  rmSync(fx.bin, { recursive: true, force: true });
  return { status, stderr, launches, probes };
}

describe("server startup fail-closed", () => {
  test("early exit redacts secrets and does not launch Playwright", async () => {
    const result = await runInner("earlydeath");
    expect(result.status).toBe(1);
    expect(result.launches).toBe(0);
    expect(result.stderr).toContain("server exited during startup");
    expect(result.stderr).toContain("fixture server exited before setup");
    expect(result.stderr).toContain("probe DATABASE_APP_URL=redacted admin unset");
    expect(result.stderr).toContain("FVOCI_LIBSQL_URL=redacted");
    expect(result.stderr).toContain("FVOCI_LIBSQL_AUTH_TOKEN=redacted");
    expect(result.stderr).toContain("FVOCI_TEST_TURSO_DATABASE_URL=redacted");
    expect(result.stderr).toContain("FVOCI_TEST_TURSO_AUTH_TOKEN=redacted");
    expect(result.stderr).toContain("libsql://redacted");
    expect(result.stderr).toContain("https://redacted");
    for (const secret of [
      "fixture-secret",
      "libsql-url-secret",
      "libsql-auth-secret",
      "turso-url-secret",
      "turso-auth-secret",
    ]) {
      expect(result.stderr).not.toContain(secret);
    }
  });

  test("a listening server that never becomes healthy stays inside 120 probes", async () => {
    const result = await runInner("neverhealthy");
    expect(result.status).toBe(1);
    expect(result.launches).toBe(0);
    expect(result.probes, result.stderr).toBe(120);
    expect(result.stderr).toContain(
      "server did not become ready within 30s (GET /api/v1/setup never succeeded)",
    );
    expect(result.stderr).toContain("probe DATABASE_APP_URL=redacted admin unset");
    expect(result.stderr).not.toContain("fixture-secret");
    expect(result.stderr).not.toContain("libsql-url-secret");
  });
});
