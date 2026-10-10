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
  SECRET_ENV,
  specConfig,
} from "./realm.ts";
import {
  LineSplitter,
  REDACTION_CASES,
  redactLine,
  forgetSecrets,
  knownSecrets,
  registerConfigSecrets,
  scrubText,
  selftestFailures,
} from "./redact.ts";
import { ENV, cli, failure, secret, ADMIN_TOKEN } from "./fixtures.ts";

let dir = "";
beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "kc-e2e-test."));
});
afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
});

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

  test("known secrets go first, longest first, raw or JSON-escaped", () => {
    forgetSecrets();
    registerConfigSecrets({
      secrets: ["abcdefgh", "abcdefghijkl"],
      users: { a: { password: 'pw-"0123é' } },
      admin: { password: "admin-pass-0" },
    });
    const secrets = knownSecrets();
    expect(secrets).toEqual(["abcdefghijkl", "admin-pass-0", 'pw-"0123é', "abcdefgh"]);
    expect(redactLine("x abcdefghijkl abcdefgh y\n", secrets)).toBe(
      "x <redacted-secret> <redacted-secret> y\n",
    );
    expect(scrubText('{"p": "pw-\\"0123\\u00e9", "q": "pw-\\"0123é"} admin-pass-0')).toBe(
      '{"p": "<redacted-secret>", "q": "<redacted-secret>"} <redacted-secret>',
    );
    forgetSecrets();
  });

  test("a short secret is refused by name, never left out", () => {
    forgetSecrets();
    expect(() => {
      registerConfigSecrets({ secrets: ["7chars!"], users: {} });
    }).toThrow("config secrets.0 is shorter than 8 characters");
    // Named by position, and every usable secret is registered first.
    expect(() => {
      registerConfigSecrets({
        secrets: [],
        users: { "user-key-0123": { password: 3 } },
        admin: { password: "admin-pass-0" },
      });
    }).toThrow("config users.0.password is not a string");
    expect(knownSecrets()).toEqual(["admin-pass-0"]);
    expect(() => {
      registerConfigSecrets({ secrets: "abcdefgh", users: {} });
    }).toThrow("config secrets is not a list");
    forgetSecrets();
    expect(() => {
      registerConfigSecrets({ users: {}, fvoci: "abcdefgh", admin: { password: "admin-pass-1" } });
    }).toThrow("config fvoci is not an object");
    expect(knownSecrets()).toEqual(["admin-pass-1"]);
    expect(() => {
      registerConfigSecrets({ secrets: [] });
    }).toThrow("config has no users");
    forgetSecrets();
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
    expect(() => renderTemplate('{"a": "@@X@@", "b": "@@A@@"}', { X: "1" })).toThrow(
      "template references an unknown placeholder",
    );
  });

  test("a value is written as JSON string content and cannot add members", () => {
    const value = 'q","injected":"r\\';
    expect(JSON.parse(renderTemplate('{"a": "@@X@@"}', { X: value }))).toEqual({ a: value });
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

  test("SECRET_ENV is every secret the runner passes in the environment", () => {
    const script = readFileSync(
      join(import.meta.dir, "../../scripts/keycloak-oidc-e2e.sh"),
      "utf8",
    );
    const body = /\nwith_secrets\(\) \{\n([\s\S]*?)\n\}\n/.exec(script)?.[1] ?? "";
    const passed = [...body.matchAll(/\b(KC_[A-Z0-9_]+)=/g)].map((match) => match[1] as string);
    expect(passed.length).toBeGreaterThan(0);
    expect([...SECRET_ENV].sort()).toEqual(passed.sort());
    expect([...SECRET_ENV].sort()).toEqual(Object.keys(ENV).sort());
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
          ? json(200, { access_token: ADMIN_TOKEN })
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
      if (request.headers.get("authorization") !== `Bearer ${ADMIN_TOKEN}`) return json(401, {});
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
    const bad = { ...config, admin: { username: "admin", password: "wrong-password-0" } };
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
    // Named by a fixed label and the code: the fs message would quote the path.
    expect(again.stderr).toBe("keycloak e2e: cannot create the config (EEXIST)\n");
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

  test("render keeps a secret with quotes inside its own JSON string", async () => {
    const out = join(dir, "quoted-realm.json");
    const template = join(import.meta.dir, "../../scripts/keycloak/realm.template.json");
    const password = 'x","enabled":false,"y":"z\\';
    const result = await cli(["render", template, out], {
      env: { ...ENV, KC_E2E_PASSWORD_ALICE: password },
    });
    expect(result.code).toBe(0);
    const realm = JSON.parse(readFileSync(out, "utf8")) as {
      users: { username: string; enabled: boolean; credentials: { value: string }[] }[];
    };
    const alice = realm.users.find((user) => user.username === "alice");
    expect(alice?.enabled).toBe(true);
    expect(alice?.credentials[0]?.value).toBe(password);
  });

  test("render and render-sso never echo a placeholder name", async () => {
    // A secret the realm does not use, shaped like a placeholder name: the
    // refusal must not quote it.
    const template = join(dir, "placeholder.template.json");
    const realmFile = join(dir, "placeholder-realm.json");
    for (const name of [
      "KC_BOOTSTRAP_ADMIN_PASSWORD",
      "KC_E2E_WRONG_SECRET",
      "KC_E2E_FVOCI_OWNER_PASSWORD",
    ]) {
      const canary = `${name.replaceAll("2", "_")}_CANARY`;
      writeFileSync(template, `{"a": "@@${canary}@@"}`);
      for (const [command, out] of [
        ["render", realmFile],
        ["render-sso", dir],
      ] as const) {
        const result = await cli([command, template, out], { env: { ...ENV, [name]: canary } });
        expect(result.code).toBe(1);
        expect(result.stdout).toBe("");
        expect(result.stderr).toBe("keycloak e2e: template references an unknown placeholder\n");
      }
      expect(() => statSync(realmFile)).toThrow();
      expect(() => statSync(join(dir, "fvoci-e2e-ws-a-realm.json"))).toThrow();
    }
  });

  test("every per-run secret in the environment is registered, used or not", async () => {
    // `redact` without a config reads none of them: only the registration
    // made before any command runs can scrub them.
    const input = `${Object.values(ENV).join(" ")}\n`;
    const result = await cli(["redact"], { env: ENV, stdin: input });
    expect(result.code).toBe(0);
    expect(result.stdout).toBe(
      `${Object.values(ENV)
        .map(() => "<redacted-secret>")
        .join(" ")}\n`,
    );
    // An unused secret that cannot be scrubbed is refused before anything is written.
    const out = join(dir, "short-unused-realm.json");
    const template = join(import.meta.dir, "../../scripts/keycloak/realm.template.json");
    const refused = await cli(["render", template, out], {
      env: { ...ENV, KC_E2E_WRONG_SECRET: "7chars!" },
    });
    expect(refused.code).toBe(1);
    expect(refused.stderr).toBe(
      "keycloak e2e: environment variable KC_E2E_WRONG_SECRET is shorter than 8 characters\n",
    );
    expect(() => statSync(out)).toThrow();
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

  test("redact with a config that cannot be read stops before stdin", async () => {
    // The runner reads exit 0 as "the config's secrets were applied".
    const input = `a ${secret("KC_BOOTSTRAP_ADMIN_PASSWORD")} b\n`;
    const result = await cli(["redact", join(dir, "no-such-config.json")], { stdin: input });
    expect(result.code).toBe(1);
    expect(result.stdout).toBe("");
    expect(result.stderr).toBe("keycloak e2e: cannot read the config (ENOENT)\n");
    // Without a path only the built-in rules apply.
    const plain = await cli(["redact"], { stdin: "GET /cb?code=9f2c.aa-11\n" });
    expect(plain.code).toBe(0);
    expect(plain.stdout).toBe("GET /cb?code=<redacted>\n");
  });

  test("redact refuses a secret that a line break or control character would split", async () => {
    // A redacted line is written as soon as it ends: a secret spanning two
    // lines would leave both halves in the output.
    const halves = ["0123abcd4567", "89ef0123abcd"];
    const config = join(dir, "redact-multiline.json");
    for (const separator of ["\n", "\r", "\x1b", "\u2028"]) {
      writeFileSync(config, JSON.stringify({ secrets: [halves.join(separator)], users: {} }));
      const result = await cli(["redact", config], { stdin: `${halves.join("\n")}\n` });
      expect(result.code).toBe(1);
      expect(result.stdout).toBe("");
      expect(result.stderr).toBe(
        "keycloak e2e: config secrets.0 contains a line break or control character\n",
      );
    }
  });

  test("unknown commands and arities exit 1", async () => {
    for (const args of [["nope"], ["render", "a"], ["selftest", "x"], []]) {
      expect((await cli(args)).code).toBe(1);
    }
    expect((await cli(["selftest"])).stdout).toBe("redaction selftest: 27 cases ok\n");
  });
});
