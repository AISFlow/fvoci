// verify through the CLI against a fake Keycloak that answers every
// read-back as imported: refusals, SSO configs, and that no known secret
// (config, environment or the admin token) reaches stdout or stderr.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expectedUsers, pyDumps, specConfig, type Json } from "./realm.ts";
import { ADMIN_TOKEN, ENV, KNOWN_REALMS, cli, secret } from "./fixtures.ts";

let dir = "";
beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "kc-e2e-test."));
});
afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
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
          ? json(200, { access_token: ADMIN_TOKEN })
          : json(401, { error: "invalid_grant" });
      }
      if (url.pathname.endsWith("/protocol/openid-connect/token")) {
        return new Response(grantBody, {
          status: 400,
          headers: { "content-type": "application/json" },
        });
      }
      if (request.headers.get("authorization") !== `Bearer ${ADMIN_TOKEN}`) return json(401, {});
      const [realm = "", section] = url.pathname.replace("/admin/realms/", "").split("/");
      if (realm === "master") return json(200, [{ username: "admin" }]);
      if (!KNOWN_REALMS.has(realm)) return json(404, {});
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

  test("the admin access token echoed back is scrubbed", async () => {
    logoutUris = `https://h/${ADMIN_TOKEN}`;
    const result = await cli(["verify", configPath(), ssoPath()]);
    logoutUris = "+";
    expect(result.code).toBe(0);
    expect(result.stdout.includes(ADMIN_TOKEN)).toBe(false);
    expect(result.stderr.includes(ADMIN_TOKEN)).toBe(false);
    expect(result.stdout).toContain('"post.logout.redirect.uris": "<redacted-secret>"');
  });

  test("a short spec secret is refused before any request, never printed", async () => {
    const short = "7chars!";
    const config = JSON.parse(readFileSync(configPath(), "utf8")) as Record<string, unknown>;
    const path = join(dir, "verify-short.json");
    writeFileSync(path, pyDumps({ ...config, secrets: [short] }));
    logoutUris = `https://h/${short}`;
    const result = await cli(["verify", path]);
    logoutUris = "+";
    expect(result.code).toBe(1);
    expect(result.stderr).toBe("keycloak e2e: config secrets.0 is shorter than 8 characters\n");
    expect(result.stdout).toBe("");
    // The config writer refuses the same value from the environment.
    const written = await cli(["config", "http://h/realms/fvoci-e2e", join(dir, "short.json")], {
      env: { ...ENV, KC_E2E_CLIENT_SECRET: short },
    });
    expect(written.code).toBe(1);
    expect(written.stderr.includes(short)).toBe(false);
    expect(written.stderr).toContain("KC_E2E_CLIENT_SECRET is shorter than 8 characters");
  });

  test("an error path that names a config value holding a secret is scrubbed", async () => {
    const canary = secret("KC_E2E_CLIENT_SECRET");
    const sso = JSON.parse(readFileSync(ssoPath(), "utf8")) as { realms: Json[] };
    const first = sso.realms[0] ?? {};
    const path = join(dir, "verify-sso-canary.json");
    writeFileSync(path, pyDumps({ realms: [{ ...first, realm: canary }] }));
    // The fake answers 404 for that realm: adminGet's error names the path.
    const result = await cli(["verify", configPath(), path]);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain("HTTP 404");
    expect(result.stderr.includes(canary)).toBe(false);
    expect(result.stdout.includes(canary)).toBe(false);
  });
});
