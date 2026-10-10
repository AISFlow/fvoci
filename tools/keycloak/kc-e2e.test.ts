// Controls of the Keycloak e2e helper: redaction, rendering and configs,
// the read-back checks against a fake admin API, and the CLI contract
// (exit codes, file modes, stdin streaming) the runner script relies on.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ready, verify } from "./admin.ts";
import {
  clientSummary,
  eventReport,
  groupSummary,
  pyDumps,
  pyDumpsIndented,
  realmSummary,
  expectedUsers,
  renderTemplate,
  specConfig,
} from "./realm.ts";
import {
  LineSplitter,
  REDACTION_CASES,
  redactLine,
  secretsOf,
  selftestFailures,
} from "./redact.ts";

const HELPER = join(import.meta.dir, "kc-e2e.ts");
const ENV: Record<string, string> = {
  KC_BOOTSTRAP_ADMIN_PASSWORD: "admin-pass-0123456789",
  KC_E2E_CLIENT_SECRET: "client-secret-0123456789",
  KC_E2E_WRONG_SECRET: "wrong-secret-0123456789",
  KC_E2E_FVOCI_OWNER_PASSWORD: "owner-pass-0123",
  KC_E2E_FVOCI_MEMBER_PASSWORD: "member-pass-0123",
  KC_E2E_SSO_A_CLIENT_SECRET: "sso-a-secret-0123",
  KC_E2E_SSO_A_PASSWORD: "sso-a-pass-0123",
  KC_E2E_SSO_B_CLIENT_SECRET: "sso-b-secret-0123",
  KC_E2E_SSO_B_PASSWORD: "sso-b-pass-0123",
};
for (const user of ["ALICE", "BOB", "CAROL", "MALLORY", "ERIN", "TINA"]) {
  ENV[`KC_E2E_PASSWORD_${user}`] = `${user.toLowerCase()}-pass-0123`;
}

const secret = (name: string) => ENV[name] ?? "";

/** The message a promise rejects with ("" when it resolves). */
async function failure(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
    return "";
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

let dir = "";
beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "kc-e2e-test."));
});
afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
});

async function cli(args: string[], options: { env?: Record<string, string>; stdin?: string } = {}) {
  const child = Bun.spawn([process.execPath, HELPER, ...args], {
    env: { PATH: process.env.PATH ?? "", ...options.env },
    stdin: options.stdin === undefined ? "ignore" : new TextEncoder().encode(options.stdin),
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { stdout, stderr, code };
}

describe("redaction", () => {
  test("every selftest case holds", () => {
    expect(selftestFailures()).toEqual([]);
    expect(REDACTION_CASES.length).toBe(27);
  });

  test("Unicode word boundaries and whitespace follow Python", () => {
    // é is a word character: no key starts inside "écode".
    expect(redactLine("écode=SECRET", [])).toBe("écode=SECRET");
    // U+00A0 is whitespace for Bearer; \x1c ends a URL parameter value.
    expect(redactLine("Bearer\u00a0tok", [])).toBe("Bearer <redacted>");
    expect(redactLine("code=abc1\x1cnext", [])).toBe("code=<redacted>\x1cnext");
  });

  test("known secrets go first, longest first", () => {
    const config = {
      secrets: ["abcdefgh", "abcdefghijkl", "short"],
      users: { a: { password: "pw-0123456" } },
    };
    const secrets = secretsOf(config);
    expect(secrets).toEqual(["abcdefghijkl", "pw-0123456", "abcdefgh"]);
    expect(redactLine("x abcdefghijkl pw-0123456 short\n", secrets)).toBe(
      "x <redacted-secret> <redacted-secret> short\n",
    );
  });

  test("a config without users is refused", () => {
    expect(() => secretsOf({ secrets: [] })).toThrow("config has no users");
  });

  test("lines split at \\n only, the tail waits for the next chunk", () => {
    const splitter = new LineSplitter();
    expect(splitter.push("a\r\nb")).toEqual(["a\r\n"]);
    expect(splitter.push("c\n\nd")).toEqual(["bc\n", "\n"]);
    expect(splitter.end()).toEqual(["d"]);
    expect(splitter.end()).toEqual([]);
  });
});

describe("realm files and configs", () => {
  test("placeholders are all filled or the template is refused", () => {
    expect(renderTemplate('{"a": "@@X@@"}', { X: "1" })).toBe('{"a": "1"}');
    expect(() => renderTemplate('{"a": "@@Z@@", "b": "@@A@@"}', {})).toThrow(
      'template placeholders without a value: ["A","Z"]',
    );
  });

  test("an invalid rendering never quotes the secret", () => {
    let message = "";
    try {
      renderTemplate("{@@S@@}", { S: "TOPSECRET" });
    } catch (error) {
      message = (error as Error).message;
    }
    expect(message).toBe("rendered template is not valid JSON");
  });

  test("the spec config lists every secret", () => {
    const config = specConfig("http://127.0.0.1:1/realms/fvoci-e2e", ENV);
    expect(config.keycloakOrigin).toBe("http://127.0.0.1:1");
    expect(config.secrets).toHaveLength(9);
    expect(() => specConfig("x", { ...ENV, KC_E2E_WRONG_SECRET: undefined })).toThrow(
      "missing environment variable KC_E2E_WRONG_SECRET",
    );
  });

  test("JSON is written as Python's json module writes it", () => {
    expect(pyDumps({ a: [1, "é"], b: null, c: { d: true } })).toBe(
      '{"a": [1, "\\u00e9"], "b": null, "c": {"d": true}}',
    );
    expect(pyDumpsIndented({ a: [], b: "\u007f" })).toBe('{\n  "a": [],\n  "b": "\\u007f"\n}');
  });
});

describe("read-back checks", () => {
  test("client settings that differ are problems", () => {
    const problems: string[] = [];
    const summary = clientSummary(
      [
        {
          clientId: "c",
          publicClient: true,
          attributes: { "pkce.code.challenge.method": "plain" },
        },
      ],
      "r",
      "c",
      problems,
    );
    // Named without the value read back; the report keeps only a marker.
    expect(problems).toContain("r client publicClient: mismatch");
    expect(problems).toContain("r client pkce.code.challenge.method: mismatch");
    expect(problems).toContain("r client redirectUris: mismatch");
    expect(summary["pkce.code.challenge.method"]).toBe("<mismatch>");
    expect(summary.clientId).toBe("c");
    const none: string[] = [];
    expect(clientSummary([], "r", "c", none)).toEqual({});
    expect(none).toEqual(["r: 0 clients named c"]);
  });

  test("users are compared as sorted rows", () => {
    const problems: string[] = [];
    const users = expectedUsers()
      .reverse()
      .map(([username, email, emailVerified, requiredActions]) => ({
        username,
        email,
        emailVerified,
        requiredActions,
      }));
    const rep = {
      sslRequired: "external",
      registrationAllowed: false,
      eventsEnabled: true,
      eventsListeners: [],
    };
    const summary = realmSummary(rep, users, "r", expectedUsers(), problems);
    expect(problems).toEqual([]);
    expect((summary.users as unknown[][])[0]?.[0]).toBe("alice");
    realmSummary(
      { ...rep, eventsListeners: ["jboss-logging"] },
      users.slice(1),
      "r",
      expectedUsers(),
      problems,
    );
    expect(problems).toHaveLength(2);
  });

  test("events keep only listed details, sorted by time, with counts", () => {
    const report = eventReport([
      { time: 2, type: "LOGIN", details: { username: "a", code_id: "SECRET" }, sessionId: "S" },
      { time: 1, type: "LOGIN_ERROR", error: "invalid_user_credentials", details: null },
      { time: 3, type: "LOGIN_ERROR", error: "" },
    ]);
    expect(report).toEqual({
      counts: { LOGIN: 1, LOGIN_ERROR: 1, "LOGIN_ERROR invalid_user_credentials": 1 },
      events: [
        { type: "LOGIN_ERROR", clientId: null, error: "invalid_user_credentials", details: {} },
        { type: "LOGIN", clientId: null, error: null, details: { username: "a" } },
        { type: "LOGIN_ERROR", clientId: null, error: "", details: {} },
      ],
    });
  });

  test("group summary: Playwright stats, else the last cargo result line", () => {
    expect(groupSummary({ stats: { expected: 3, unexpected: 1 } }, undefined)).toBe(
      "passed 3, failed 1, flaky 0, skipped 0",
    );
    expect(groupSummary(undefined, "x\ntest result: FAILED.\ntest result: ok. 1 passed\n")).toBe(
      "test result: ok. 1 passed",
    );
    expect(groupSummary(undefined, "  test result: indented\n")).toBe("no report");
    expect(groupSummary(undefined, undefined)).toBe("no report");
  });
});

describe("fake Keycloak", () => {
  let jwks: unknown = { keys: [{ use: "sig", alg: "RS256" }] };
  let tokenSeen: string | null = null;
  const server = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    async fetch(request) {
      const url = new URL(request.url);
      const issuer = `${url.origin}/realms/fvoci-e2e`;
      const json = (status: number, body: unknown) => Response.json(body, { status });
      if (url.pathname.endsWith("/.well-known/openid-configuration")) {
        return json(200, {
          issuer,
          authorization_endpoint: `${issuer}/auth`,
          token_endpoint: `${issuer}/token`,
          jwks_uri: `${issuer}/certs`,
        });
      }
      if (url.pathname.endsWith("/certs")) return json(200, jwks);
      if (url.pathname === "/realms/master/protocol/openid-connect/token") {
        const form = new URLSearchParams(await request.text());
        return form.get("password") === ENV.KC_BOOTSTRAP_ADMIN_PASSWORD
          ? json(200, { access_token: "ADMIN" })
          : json(401, { error: "invalid_grant" });
      }
      if (url.pathname.endsWith("/protocol/openid-connect/token")) {
        // A redirect must not be followed with the client secret.
        return new Response(null, {
          status: 307,
          headers: { location: `${url.origin}/elsewhere` },
        });
      }
      if (url.pathname === "/elsewhere") {
        tokenSeen = await request.text();
        return json(200, { access_token: "leaked" });
      }
      if (request.headers.get("authorization") !== "Bearer ADMIN") return json(401, {});
      const path = url.pathname.replace("/admin/realms", "");
      if (path === "/fvoci-e2e") {
        return json(200, {
          sslRequired: "external",
          registrationAllowed: false,
          eventsEnabled: true,
          eventsListeners: [],
        });
      }
      if (path === "/fvoci-e2e/users") {
        return json(
          200,
          expectedUsers().map(([username, email, emailVerified, requiredActions]) => ({
            username,
            email,
            emailVerified,
            requiredActions,
          })),
        );
      }
      if (path === "/fvoci-e2e/clients") return json(200, []);
      if (path === "/fvoci-e2e/authentication/required-actions") {
        return json(200, [{ alias: "TERMS_AND_CONDITIONS", enabled: true }]);
      }
      if (path === "/master/users") return json(200, [{ username: "admin" }]);
      return json(404, {});
    },
  });
  afterAll(() => server.stop(true));
  const origin = `http://127.0.0.1:${String(server.port)}`;

  test("ready needs an RS256 signing key", async () => {
    await ready(`${origin}/realms/fvoci-e2e`);
    jwks = { keys: [{ use: "enc", alg: "RS256" }] };
    expect(await failure(ready(`${origin}/realms/fvoci-e2e`))).toBe(
      "jwks has no RS256 signing key yet",
    );
    jwks = { keys: [{ use: "sig", alg: "RS256" }] };
    expect(await failure(ready(`${origin}/realms/other`))).toBe(
      "discovery issuer differs from the configured issuer",
    );
  });

  test("verify reports problems and never follows a credential redirect", async () => {
    const config = specConfig(`${origin}/realms/fvoci-e2e`, ENV);
    const { report, problems } = await verify(config);
    expect(problems).toContain("fvoci-e2e: 0 clients named fvoci-e2e");
    // The 307 counts as "not refused" (status < 400), and nothing reached the target.
    expect(problems).toContain(`${origin}/realms/fvoci-e2e: password grant was not refused`);
    expect(tokenSeen).toBeNull();
    expect(report.masterRealmUsers).toEqual(["admin"]);
    const bad = { ...config, admin: { username: "admin", password: "wrong" } };
    expect(await failure(verify(bad))).toBe("admin token HTTP 401");
    // A missing credential stops the check instead of posting "undefined".
    const noAlice = { ...config, users: {} };
    expect(await failure(verify(noAlice))).toBe("config lacks users.alice.password");
  });
});

describe("CLI", () => {
  test("config is created mode 600 and never overwritten", async () => {
    const out = join(dir, "kc.json");
    const first = await cli(["config", "http://127.0.0.1:1/realms/fvoci-e2e", out], { env: ENV });
    expect(first.code).toBe(0);
    expect(statSync(out).mode & 0o777).toBe(0o600);
    const again = await cli(["config", "http://127.0.0.1:1/realms/fvoci-e2e", out], { env: ENV });
    expect(again.code).toBe(1);
    expect(again.stderr).toStartWith("keycloak e2e: ");
  });

  test("render refuses a missing secret and writes nothing", async () => {
    const out = join(dir, "realm.json");
    const template = join(import.meta.dir, "../../scripts/keycloak/realm.template.json");
    const withoutTina = { ...ENV };
    delete withoutTina.KC_E2E_PASSWORD_TINA;
    const missing = await cli(["render", template, out], { env: withoutTina });
    expect(missing.code).toBe(1);
    expect(missing.stderr).toBe(
      "keycloak e2e: missing environment variable KC_E2E_PASSWORD_TINA\n",
    );
    expect(() => statSync(out)).toThrow();
    const rendered = await cli(["render", template, out], { env: ENV });
    expect(rendered.code).toBe(0);
    expect(statSync(out).mode & 0o777 & ~0o644).toBe(0);
    expect(readFileSync(out, "utf8")).toContain(secret("KC_E2E_PASSWORD_TINA"));
  });

  test("redact streams stdin with the config's secrets", async () => {
    const config = join(dir, "redact.json");
    writeFileSync(config, pyDumps(specConfig("http://h/realms/fvoci-e2e", ENV)));
    const input = `a ${secret("KC_E2E_CLIENT_SECRET")} b\nGET /cb?code=9f2c.aa-11\ntail ${secret("KC_E2E_PASSWORD_ALICE")}`;
    const result = await cli(["redact", config], { stdin: input });
    expect(result.code).toBe(0);
    expect(result.stdout).toBe(
      "a <redacted-secret> b\nGET /cb?code=<redacted>\ntail <redacted-secret>",
    );
    expect(readFileSync(config, "utf8")).toContain(secret("KC_E2E_CLIENT_SECRET"));
  });

  test("unknown commands and arities exit 1", async () => {
    for (const args of [["nope"], ["render", "a"], ["selftest", "x"], []]) {
      expect((await cli(args)).code).toBe(1);
    }
    expect((await cli(["selftest"])).stdout).toBe("redaction selftest: 27 cases ok\n");
  });
});

// A Keycloak that answers every read-back as imported, so a single changed
// answer is the only reason for a verdict.
describe("verify against a correct fake Keycloak", () => {
  let grantBody = JSON.stringify({ error: "unauthorized_client" });
  let pkce = "S256";
  let logoutUris = "+";
  const validClient = (clientId: string) => ({
    clientId,
    publicClient: false,
    bearerOnly: false,
    clientAuthenticatorType: "client-secret",
    standardFlowEnabled: true,
    implicitFlowEnabled: false,
    directAccessGrantsEnabled: false,
    serviceAccountsEnabled: false,
    consentRequired: false,
    fullScopeAllowed: false,
    frontchannelLogout: false,
    redirectUris: [],
    webOrigins: [],
    defaultClientScopes: ["basic"],
    optionalClientScopes: ["profile", "email"],
    attributes: {
      "pkce.code.challenge.method": pkce,
      "oauth2.device.authorization.grant.enabled": "false",
      "oidc.ciba.grant.enabled": "false",
      "standard.token.exchange.enabled": "false",
      "post.logout.redirect.uris": logoutUris,
    },
  });
  const usersOf = (realm: string) => {
    if (realm === "fvoci-e2e") {
      return expectedUsers().map(([username, email, emailVerified, requiredActions]) => ({
        username,
        email,
        emailVerified,
        requiredActions,
      }));
    }
    const user = realm.replace("fvoci-e2e-", "");
    return [{ username: user, email: `kc-${user}@example.com`, emailVerified: true }];
  };
  const server = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    async fetch(request) {
      const url = new URL(request.url);
      const json = (status: number, body: unknown) => Response.json(body, { status });
      if (url.pathname === "/realms/master/protocol/openid-connect/token") {
        const form = new URLSearchParams(await request.text());
        return form.get("password") === secret("KC_BOOTSTRAP_ADMIN_PASSWORD")
          ? json(200, { access_token: "ADMIN" })
          : json(401, { error: "invalid_grant" });
      }
      if (url.pathname.endsWith("/protocol/openid-connect/token")) {
        return new Response(grantBody, {
          status: 400,
          headers: { "content-type": "application/json" },
        });
      }
      if (request.headers.get("authorization") !== "Bearer ADMIN") return json(401, {});
      const [realm = "", section] = url.pathname.replace("/admin/realms/", "").split("/");
      if (realm === "master") return json(200, [{ username: "admin" }]);
      if (section === undefined) {
        return json(200, {
          realm,
          sslRequired: "external",
          registrationAllowed: false,
          eventsEnabled: true,
          eventsListeners: [],
        });
      }
      if (section === "users") return json(200, usersOf(realm));
      if (section === "clients") {
        return json(200, [validClient(url.searchParams.get("clientId") ?? "")]);
      }
      if (section === "authentication") {
        return json(200, [{ alias: "TERMS_AND_CONDITIONS", enabled: true }]);
      }
      return json(404, {});
    },
  });
  afterAll(() => server.stop(true));
  const origin = `http://127.0.0.1:${String(server.port)}`;
  const configPath = () => join(dir, "verify-kc.json");
  const ssoPath = () => join(dir, "verify-sso.json");
  beforeAll(() => {
    writeFileSync(configPath(), pyDumps(specConfig(`${origin}/realms/fvoci-e2e`, ENV)));
    const realms = ["a", "b"].map((key) => ({
      realm: `fvoci-e2e-ws-${key}`,
      issuer: `${origin}/realms/fvoci-e2e-ws-${key}`,
      clientId: "fvoci-ws",
      clientSecret: secret(`KC_E2E_SSO_${key.toUpperCase()}_CLIENT_SECRET`),
      username: `ws-${key}`,
      password: secret(`KC_E2E_SSO_${key.toUpperCase()}_PASSWORD`),
      email: `kc-ws-${key}@example.com`,
    }));
    writeFileSync(ssoPath(), pyDumps({ realms }));
  });

  test("the baseline passes, with and without the SSO realms", async () => {
    expect((await cli(["verify", configPath()])).code).toBe(0);
    expect((await cli(["verify", configPath(), ssoPath()])).code).toBe(0);
  });

  test("a 400 answer that is not an OAuth error object is not a refusal", async () => {
    for (const body of ["null", "[]", "false", "17", "{}", '{"error": 3}', ""]) {
      grantBody = body;
      const result = await cli(["verify", configPath()]);
      expect({ body, code: result.code }).toEqual({ body, code: 1 });
      expect(result.stderr).toContain("password grant was not refused");
    }
    grantBody = JSON.stringify({ error: "unauthorized_client" });
  });

  test("a given SSO config without its realms stops the run", async () => {
    const cases: [string, string][] = [
      ["empty", "{}"],
      ["no-realms", '{"realms": []}'],
      ["realm-lacks-a-secret", '{"realms": [{"realm": "fvoci-e2e-ws-a"}]}'],
    ];
    for (const [name, content] of cases) {
      const path = join(dir, `sso-${name}.json`);
      writeFileSync(path, content);
      const result = await cli(["verify", configPath(), path]);
      expect({ name, code: result.code }).toEqual({ name, code: 1 });
      expect(result.stderr).toStartWith("keycloak e2e: config lacks realms");
    }
  });

  test("a mismatch never prints a known secret (canary in a read-back field)", async () => {
    const canary = secret("KC_E2E_CLIENT_SECRET");
    for (const value of [canary, `x-${canary}-y`]) {
      pkce = value;
      const result = await cli(["verify", configPath(), ssoPath()]);
      expect(result.code).toBe(1);
      expect(result.stderr).toContain("fvoci-e2e client pkce.code.challenge.method: mismatch");
      expect(result.stderr.includes(canary)).toBe(false);
      expect(result.stdout.includes(canary)).toBe(false);
      expect(result.stdout).toContain('"pkce.code.challenge.method": "<mismatch>"');
    }
    pkce = "S256";
  });

  test("an unchecked read-back field never carries a known secret into the report", async () => {
    const canary = secret("KC_E2E_SSO_A_CLIENT_SECRET");
    logoutUris = `https://h/${canary}`;
    const result = await cli(["verify", configPath(), ssoPath()]);
    logoutUris = "+";
    expect(result.code).toBe(0);
    expect(result.stdout.includes(canary)).toBe(false);
    expect(result.stdout).toContain('"post.logout.redirect.uris": "<redacted-secret>"');
  });
});
