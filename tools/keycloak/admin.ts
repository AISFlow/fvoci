// Keycloak over HTTP: readiness, the admin API read-back and the event log.
import {
  CLIENT_ID,
  REALM,
  clientSummary,
  eventReport,
  expectedUsers,
  get,
  grantRefusal,
  masterUsers,
  realmSummary,
  requiredActions,
  scrubSecrets,
  type Json,
} from "./realm.ts";
import {
  HelperError,
  addSecret,
  errorName,
  knownSecrets,
  registerConfigSecrets,
} from "./redact.ts";

type Answer = { status: number; body: string };

/**
 * One request. Requests that carry credentials (a form or a bearer token)
 * never follow a redirect, so a password or token is not resent elsewhere;
 * a redirect answers with its own status.
 */
export async function http(
  method: "GET" | "POST",
  url: string,
  options: { form?: Record<string, string>; token?: string; timeoutMs?: number } = {},
): Promise<Answer> {
  const headers: Record<string, string> = {};
  if (options.token) headers.Authorization = `Bearer ${options.token}`;
  const response = await fetch(url, {
    method,
    headers,
    body: options.form === undefined ? undefined : new URLSearchParams(options.form),
    redirect: options.form === undefined && options.token === undefined ? "follow" : "manual",
    signal: AbortSignal.timeout(options.timeoutMs ?? 5000),
  });
  return { status: response.status, body: await response.text() };
}

function parse(text: string, what: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    throw new HelperError(`${what} is not JSON`);
  }
}

/** A string member of a config; a missing one stops the run instead of sending "undefined". */
function text(value: unknown, ...path: string[]): string {
  let member: unknown = value;
  for (const key of path) member = get(member, key);
  if (typeof member !== "string") throw new HelperError(`config lacks ${path.join(".")}`);
  return member;
}

/** Discovery answers 200 with exactly this issuer and its JWKS has an RS256 signing key. */
export async function ready(issuer: string): Promise<void> {
  let discovery: Answer;
  try {
    discovery = await http("GET", `${issuer}/.well-known/openid-configuration`, {
      timeoutMs: 3000,
    });
  } catch (error) {
    throw new HelperError(`discovery not reachable (${errorName(error)})`);
  }
  if (discovery.status !== 200) throw new HelperError(`discovery HTTP ${String(discovery.status)}`);
  const doc = parse(discovery.body, "discovery");
  if (get(doc, "issuer") !== issuer)
    throw new HelperError("discovery issuer differs from the configured issuer");
  for (const key of ["authorization_endpoint", "token_endpoint", "jwks_uri"]) {
    const value = get(doc, key);
    if (typeof value !== "string" || !value.startsWith(`${issuer}/`)) {
      throw new HelperError(`discovery ${key} is not under the issuer`);
    }
  }
  let jwks: Answer;
  try {
    jwks = await http("GET", get(doc, "jwks_uri") as string, { timeoutMs: 3000 });
  } catch (error) {
    throw new HelperError(`jwks not reachable (${errorName(error)})`);
  }
  if (jwks.status !== 200) throw new HelperError(`jwks HTTP ${String(jwks.status)}`);
  const keys = get(parse(jwks.body, "jwks"), "keys");
  const signing = (Array.isArray(keys) ? keys : []).some(
    (key) => get(key, "use") === "sig" && get(key, "alg") === "RS256",
  );
  if (!signing) throw new HelperError("jwks has no RS256 signing key yet");
}

type Admin = { origin: string; token: string };

async function adminSession(config: Json): Promise<Admin> {
  const origin = text(config, "keycloakOrigin");
  const answer = await http("POST", `${origin}/realms/master/protocol/openid-connect/token`, {
    form: {
      grant_type: "password",
      client_id: "admin-cli",
      username: text(config, "admin", "username"),
      password: text(config, "admin", "password"),
    },
  });
  if (answer.status !== 200) throw new HelperError(`admin token HTTP ${String(answer.status)}`);
  const token = get(parse(answer.body, "admin token answer"), "access_token");
  if (typeof token !== "string") throw new HelperError("admin token answer has no access_token");
  // A credential of this process from here on: scrubbed from all output.
  addSecret(token, "admin access token");
  return { origin, token };
}

async function adminGet(admin: Admin, path: string): Promise<unknown> {
  const answer = await http("GET", `${admin.origin}/admin/realms${path}`, { token: admin.token });
  if (answer.status !== 200)
    throw new HelperError(`admin GET ${path} HTTP ${String(answer.status)}`);
  return parse(answer.body, `admin GET ${path}`);
}

/** Grants other than the authorization code are refused, with the real secret. */
async function grantRefusals(
  issuer: string,
  clientId: string,
  secret: string,
  username: string,
  password: string,
  problems: string[],
): Promise<Json> {
  const refusals: Json = {};
  const grants: [string, Record<string, string>][] = [
    ["password", { username, password }],
    ["client_credentials", {}],
  ];
  for (const [grant, extra] of grants) {
    const answer = await http("POST", `${issuer}/protocol/openid-connect/token`, {
      form: {
        grant_type: grant,
        client_id: clientId,
        client_secret: secret,
        scope: "openid",
        ...extra,
      },
    });
    const body = answer.body === "" ? {} : parse(answer.body, `${grant} grant answer`);
    refusals[grant] = grantRefusal(issuer, grant, answer.status, body, problems);
  }
  return refusals;
}

async function realmRead(
  admin: Admin,
  realm: string,
  expected: ReturnType<typeof expectedUsers>,
  problems: string[],
) {
  const rep = await adminGet(admin, `/${realm}`);
  const users = await adminGet(admin, `/${realm}/users?max=100`);
  return realmSummary(rep, users, realm, expected, problems);
}

async function clientRead(admin: Admin, realm: string, clientId: string, problems: string[]) {
  return clientSummary(
    await adminGet(admin, `/${realm}/clients?clientId=${clientId}`),
    realm,
    clientId,
    problems,
  );
}

/** Reads the imported settings back instead of assuming the import applied. */
export async function verify(
  config: Json,
  ssoConfig?: Json,
): Promise<{ report: Json; problems: string[] }> {
  registerConfigSecrets(config);
  const ssoRealms = ssoConfig === undefined ? undefined : ssoRealmsOf(ssoConfig);
  for (const [index, realm] of (ssoRealms ?? []).entries()) {
    addSecret(realm.clientSecret, `config realms.${String(index)}.clientSecret`);
    addSecret(realm.password, `config realms.${String(index)}.password`);
  }
  const { report, problems } = await readBack(config, ssoRealms);
  // Read-back values that hold a known secret (the admin token included) are
  // replaced whole; the CLI's output boundary scrubs every write once more.
  const secrets = knownSecrets();
  return {
    report: scrubSecrets(report, secrets) as Json,
    problems: problems.map((problem) => scrubSecrets(problem, secrets) as string),
  };
}

type SsoRealm = Record<
  "realm" | "issuer" | "clientId" | "clientSecret" | "username" | "password" | "email",
  string
>;

/** The realms of a given SSO config; a missing or mistyped member stops the run. */
function ssoRealmsOf(ssoConfig: Json): SsoRealm[] {
  const realms = ssoConfig.realms;
  if (!Array.isArray(realms) || realms.length === 0) {
    throw new HelperError("config lacks realms");
  }
  return realms.map((realm: unknown, index) => {
    const out = {} as SsoRealm;
    for (const key of [
      "realm",
      "issuer",
      "clientId",
      "clientSecret",
      "username",
      "password",
      "email",
    ] as const) {
      const value = get(realm, key);
      if (typeof value !== "string" || value === "")
        throw new HelperError(`config lacks realms.${String(index)}.${key}`);
      out[key] = value;
    }
    return out;
  });
}

async function readBack(
  config: Json,
  ssoRealms: SsoRealm[] | undefined,
): Promise<{ report: Json; problems: string[] }> {
  const admin = await adminSession(config);
  const problems: string[] = [];
  const report: Json = {
    realm: await realmRead(admin, REALM, expectedUsers(), problems),
    client: await clientRead(admin, REALM, CLIENT_ID, problems),
    tokenEndpointRefusals: await grantRefusals(
      text(config, "issuer"),
      CLIENT_ID,
      text(config, "secrets", "0"),
      "alice",
      text(config, "users", "alice", "password"),
      problems,
    ),
  };
  report.requiredActions = requiredActions(
    await adminGet(admin, `/${REALM}/authentication/required-actions`),
    problems,
  );
  report.masterRealmUsers = masterUsers(await adminGet(admin, "/master/users?max=100"), problems);
  if (ssoRealms !== undefined) {
    const sso: Json = {};
    for (const realm of ssoRealms) {
      sso[realm.realm] = {
        realm: await realmRead(
          admin,
          realm.realm,
          [[realm.username, realm.email, true, []]],
          problems,
        ),
        client: await clientRead(admin, realm.realm, realm.clientId, problems),
        tokenEndpointRefusals: await grantRefusals(
          realm.issuer,
          realm.clientId,
          realm.clientSecret,
          realm.username,
          realm.password,
          problems,
        ),
      };
    }
    report.workspaceSsoRealms = sso;
  }
  return { report, problems };
}

export async function events(config: Json, realms: string[]): Promise<Json> {
  registerConfigSecrets(config);
  const admin = await adminSession(config);
  const report: Json = {};
  for (const realm of realms.length > 0 ? realms : [REALM]) {
    report[realm] = eventReport(await adminGet(admin, `/${realm}/events?max=1000`));
  }
  return report;
}
