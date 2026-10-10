// The test realms, the configs handed to the spec and the Rust test, and the
// checks on what Keycloak reports back. No I/O: callers pass the environment
// and the admin API answers.
import { HelperError, addSecret } from "./redact.ts";

export const REALM = "fvoci-e2e";
export const CLIENT_ID = "fvoci-e2e";
export const LABEL = "Keycloak E2E";
// username: [email, emailVerified, required actions]
export const USERS: Record<string, readonly [string, boolean, readonly string[]]> = {
  alice: ["kc-alice@example.com", true, []],
  bob: ["kc-bob@example.com", true, []],
  carol: ["kc-carol@example.com", true, []],
  mallory: ["kc-mallory@example.com", true, []],
  erin: ["kc-erin@example.com", false, []],
  tina: ["kc-tina@example.com", true, ["TERMS_AND_CONDITIONS"]],
};
// Workspace SSO (opt-in Rust test): two realms, i.e. two issuers, one client
// and one user each.
export const SSO_REALMS = {
  A: { realm: "fvoci-e2e-ws-a", username: "ws-a", email: "kc-ws-a@example.com" },
  B: { realm: "fvoci-e2e-ws-b", username: "ws-b", email: "kc-ws-b@example.com" },
} as const;
export const SSO_CLIENT_ID = "fvoci-ws";

export type Env = Record<string, string | undefined>;
export type Json = Record<string, unknown>;

/** A per-run secret from the environment (never echoed). */
export function need(env: Env, name: string): string {
  const value = env[name];
  if (value === undefined) throw new HelperError(`missing environment variable ${name}`);
  addSecret(value, `environment variable ${name}`);
  return value;
}

const ssoSecret = (env: Env, key: string, what: string) => need(env, `KC_E2E_SSO_${key}_${what}`);

/** The template with every `@@NAME@@` replaced; refuses unknown names and non-JSON output. */
export function renderTemplate(text: string, values: Record<string, string>): string {
  const placeholder = /@@([A-Z_]+)@@/g;
  const names = new Set([...text.matchAll(placeholder)].map((match) => match[1] as string));
  const missing = [...names].filter((name) => !Object.hasOwn(values, name)).sort(compareCodePoints);
  if (missing.length > 0) {
    throw new HelperError(`template placeholders without a value: ${JSON.stringify(missing)}`);
  }
  const rendered = text.replace(placeholder, (_whole, name: string) => values[name] as string);
  try {
    JSON.parse(rendered);
  } catch {
    // The parser's message may quote the rendered text, which holds secrets.
    throw new HelperError("rendered template is not valid JSON");
  }
  return rendered;
}

export function realmValues(env: Env): Record<string, string> {
  const values: Record<string, string> = { CLIENT_SECRET: need(env, "KC_E2E_CLIENT_SECRET") };
  for (const name of Object.keys(USERS)) {
    const key = `PASSWORD_${name.toUpperCase()}`;
    values[key] = need(env, `KC_E2E_${key}`);
  }
  return values;
}

/** [file name, placeholder values] of each workspace SSO realm. */
export function ssoRealmValues(env: Env): [string, Record<string, string>][] {
  return Object.entries(SSO_REALMS).map(([key, realm]) => [
    `${realm.realm}-realm.json`,
    {
      REALM: realm.realm,
      CLIENT_SECRET: ssoSecret(env, key, "CLIENT_SECRET"),
      USERNAME: realm.username,
      EMAIL: realm.email,
      PASSWORD: ssoSecret(env, key, "PASSWORD"),
    },
  ]);
}

export function ssoConfig(keycloakOrigin: string, env: Env): Json {
  return {
    keycloakOrigin,
    admin: { username: "admin", password: need(env, "KC_BOOTSTRAP_ADMIN_PASSWORD") },
    realms: Object.entries(SSO_REALMS).map(([key, realm]) => ({
      realm: realm.realm,
      issuer: `${keycloakOrigin}/realms/${realm.realm}`,
      clientId: SSO_CLIENT_ID,
      clientSecret: ssoSecret(env, key, "CLIENT_SECRET"),
      username: realm.username,
      password: ssoSecret(env, key, "PASSWORD"),
      email: realm.email,
    })),
  };
}

export function specConfig(issuer: string, env: Env): Json {
  return {
    issuer,
    realm: REALM,
    clientId: CLIENT_ID,
    label: LABEL,
    keycloakOrigin: issuer.split("/realms/")[0],
    admin: { username: "admin", password: need(env, "KC_BOOTSTRAP_ADMIN_PASSWORD") },
    users: Object.fromEntries(
      Object.entries(USERS).map(([name, [email]]) => [
        name,
        { username: name, email, password: need(env, `KC_E2E_PASSWORD_${name.toUpperCase()}`) },
      ]),
    ),
    // FVOCI accounts the spec creates (password sign-in).
    fvoci: {
      ownerPassword: need(env, "KC_E2E_FVOCI_OWNER_PASSWORD"),
      memberPassword: need(env, "KC_E2E_FVOCI_MEMBER_PASSWORD"),
    },
    secrets: [
      need(env, "KC_E2E_CLIENT_SECRET"),
      need(env, "KC_E2E_WRONG_SECRET"),
      need(env, "KC_BOOTSTRAP_ADMIN_PASSWORD"),
      need(env, "KC_E2E_FVOCI_OWNER_PASSWORD"),
      need(env, "KC_E2E_FVOCI_MEMBER_PASSWORD"),
      ...Object.keys(SSO_REALMS).flatMap((key) => [
        ssoSecret(env, key, "CLIENT_SECRET"),
        ssoSecret(env, key, "PASSWORD"),
      ]),
    ],
  };
}

// --- JSON as Python's json module writes it --------------------------------

// Python's ensure_ascii escapes DEL and everything above it (UTF-16 units).
const NON_ASCII = new RegExp("[\\u007f-\\uffff]", "g");
const asciiOnly = (text: string) =>
  text.replace(NON_ASCII, (char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`);

/** `json.dumps(value)` (", " and ": " separators, ASCII only). */
export function pyDumps(value: unknown): string {
  const dump = (item: unknown): string => {
    if (Array.isArray(item)) return `[${item.map(dump).join(", ")}]`;
    if (item !== null && typeof item === "object") {
      const members = Object.entries(item).map(
        ([key, member]) => `${JSON.stringify(key)}: ${dump(member)}`,
      );
      return `{${members.join(", ")}}`;
    }
    return JSON.stringify(item ?? null);
  };
  return asciiOnly(dump(value));
}

/** `json.dumps(value, indent=2)`. */
export function pyDumpsIndented(value: unknown): string {
  return asciiOnly(JSON.stringify(value, null, 2));
}

// --- Python ordering and lookups -------------------------------------------

export function compareCodePoints(a: string, b: string): number {
  const left = Array.from(a, (char) => char.codePointAt(0) ?? 0);
  const right = Array.from(b, (char) => char.codePointAt(0) ?? 0);
  for (let i = 0; i < Math.min(left.length, right.length); i++) {
    const delta = (left[i] ?? 0) - (right[i] ?? 0);
    if (delta !== 0) return delta;
  }
  return left.length - right.length;
}

/** Python's `<` over the JSON values compared here (strings, numbers, booleans, lists; None first). */
export function pyCompare(a: unknown, b: unknown): number {
  if (Array.isArray(a) && Array.isArray(b)) {
    for (let i = 0; i < Math.min(a.length, b.length); i++) {
      const delta = pyCompare(a[i], b[i]);
      if (delta !== 0) return delta;
    }
    return a.length - b.length;
  }
  if (typeof a === "string" && typeof b === "string") return compareCodePoints(a, b);
  if (a === null || a === undefined) return b === null || b === undefined ? 0 : -1;
  if (b === null || b === undefined) return 1;
  const numeric = (value: unknown) => typeof value === "number" || typeof value === "boolean";
  if (numeric(a) && numeric(b)) return Number(a) - Number(b);
  // Python refuses to order mixed types; a total order keeps the sort
  // deterministic, and the deep comparison after it still reports the row.
  return compareCodePoints(`${typeof a}:${JSON.stringify(a)}`, `${typeof b}:${JSON.stringify(b)}`);
}

export function deepEqual(a: unknown, b: unknown): boolean {
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => deepEqual(item, b[i]));
  }
  return a === b;
}

/** `dict.get(key)`: the member, or null when absent. */
export function get(value: unknown, key: string): unknown {
  return value !== null && typeof value === "object" && Object.hasOwn(value, key)
    ? (value as Json)[key]
    : null;
}

// A read-back value that differs is replaced by this marker in the report and
// named without its value in the problems: Keycloak may echo anything back,
// a per-run secret included.
export const MISMATCH = "<mismatch>";

// --- Read-back checks of the imported realms --------------------------------

export function clientSummary(
  clients: unknown,
  realm: string,
  clientId: string,
  problems: string[],
): Json {
  const list = Array.isArray(clients) ? clients : [];
  if (list.length !== 1) {
    problems.push(`${realm}: ${String(list.length)} clients named ${clientId}`);
    return {};
  }
  const client: unknown = list[0];
  const attributes = get(client, "attributes") ?? {};
  const scopes = (key: string) =>
    [...((get(client, key) as string[] | null) ?? [])].sort(compareCodePoints);
  const summary: Json = {};
  for (const key of [
    "clientId",
    "publicClient",
    "bearerOnly",
    "clientAuthenticatorType",
    "standardFlowEnabled",
    "implicitFlowEnabled",
    "directAccessGrantsEnabled",
    "serviceAccountsEnabled",
    "consentRequired",
    "fullScopeAllowed",
    "frontchannelLogout",
    "redirectUris",
    "webOrigins",
  ]) {
    summary[key] = get(client, key);
  }
  summary.defaultClientScopes = scopes("defaultClientScopes");
  summary.optionalClientScopes = scopes("optionalClientScopes");
  for (const key of [
    "pkce.code.challenge.method",
    "oauth2.device.authorization.grant.enabled",
    "oidc.ciba.grant.enabled",
    "standard.token.exchange.enabled",
    "post.logout.redirect.uris",
  ]) {
    summary[key] = get(attributes, key);
  }
  const expected: Json = {
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
    // The server's port (or workspace id) is known only later: the spec and
    // the Rust test register the exact callback URI before the first flow.
    redirectUris: [],
    webOrigins: [],
    defaultClientScopes: ["basic"],
    optionalClientScopes: ["email", "profile"],
    "pkce.code.challenge.method": "S256",
    "oauth2.device.authorization.grant.enabled": "false",
    "oidc.ciba.grant.enabled": "false",
    "standard.token.exchange.enabled": "false",
  };
  for (const [key, value] of Object.entries(expected)) {
    if (!deepEqual(summary[key], value)) {
      problems.push(`${realm} client ${key}: mismatch`);
      summary[key] = MISMATCH;
    }
  }
  return summary;
}

/** [username, email, emailVerified, required actions] rows, sorted. */
export type UserRow = [unknown, unknown, unknown, unknown[]];

export function expectedUsers(): UserRow[] {
  return Object.entries(USERS).map(([name, [email, verified, actions]]) => [
    name,
    email,
    verified,
    [...actions],
  ]);
}

export function realmSummary(
  rep: unknown,
  users: unknown,
  realm: string,
  expected: UserRow[],
  problems: string[],
): Json {
  const summary: Json = {};
  for (const key of [
    "realm",
    "enabled",
    "sslRequired",
    "registrationAllowed",
    "resetPasswordAllowed",
    "verifyEmail",
    "eventsEnabled",
    "eventsListeners",
  ]) {
    summary[key] = get(rep, key);
  }
  const wanted: [string, unknown][] = [
    ["sslRequired", "external"],
    ["registrationAllowed", false],
    ["eventsEnabled", true],
    ["eventsListeners", []],
  ];
  for (const [key, value] of wanted) {
    if (!deepEqual(summary[key], value)) {
      problems.push(`${realm} ${key}: mismatch`);
      summary[key] = MISMATCH;
    }
  }
  const actual: UserRow[] = (Array.isArray(users) ? users : [])
    .map((user): UserRow => [
      get(user, "username"),
      get(user, "email"),
      get(user, "emailVerified"),
      [...((get(user, "requiredActions") as unknown[] | null) ?? [])].sort(pyCompare),
    ])
    .sort(pyCompare);
  const want = [...expected].sort(pyCompare);
  if (deepEqual(actual, want)) {
    summary.users = actual;
  } else {
    problems.push(`${realm} users: mismatch`);
    summary.users = MISMATCH;
  }
  return summary;
}

/** The grant's refusal record; a grant that was not refused is a problem. */
export function grantRefusal(
  issuer: string,
  grant: string,
  status: number,
  answer: unknown,
  problems: string[],
): Json {
  // A refusal is an OAuth error answer (RFC 6749 5.2): an error status and a
  // JSON object with a string `error` and no token. Anything else is not
  // evidence that the grant is disabled.
  const isObject = answer !== null && typeof answer === "object" && !Array.isArray(answer);
  const error = isObject ? get(answer, "error") : null;
  const refused =
    status >= 400 &&
    isObject &&
    typeof error === "string" &&
    !Object.hasOwn(answer, "access_token");
  if (!refused) problems.push(`${issuer}: ${grant} grant was not refused`);
  return { status, error: refused ? error : MISMATCH };
}

export function requiredActions(actions: unknown, problems: string[]): Json {
  const out: Json = {};
  for (const action of Array.isArray(actions) ? actions : []) {
    out[String(get(action, "alias"))] = get(action, "enabled");
  }
  if (out.TERMS_AND_CONDITIONS !== true) problems.push("TERMS_AND_CONDITIONS is not enabled");
  return out;
}

export function masterUsers(users: unknown, problems: string[]): unknown {
  const names = (Array.isArray(users) ? users : [])
    .map((user) => get(user, "username"))
    .sort(pyCompare);
  if (deepEqual(names, ["admin"])) return names;
  problems.push("master realm users: mismatch");
  return MISMATCH;
}

/**
 * The report with every string (and key) that contains a known secret
 * replaced: unchecked read-back fields are printed as they are.
 */
export function scrubSecrets(value: unknown, secrets: readonly string[]): unknown {
  const clean = (text: string) =>
    secrets.some((secret) => text.includes(secret)) ? "<redacted-secret>" : text;
  if (typeof value === "string") return clean(value);
  if (Array.isArray(value)) return value.map((item) => scrubSecrets(item, secrets));
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, member]) => [clean(key), scrubSecrets(member, secrets)]),
    );
  }
  return value;
}

// --- Events and the run summary ---------------------------------------------

const EVENT_DETAILS = [
  "grant_type",
  "client_auth_method",
  "auth_method",
  "redirect_uri",
  "response_type",
  "scope",
  "username",
  "custom_required_action",
  "reason",
];

/** One realm's events without ids, sessions or tokens, and their counts. */
export function eventReport(raw: unknown): Json {
  const events: unknown[] = Array.isArray(raw) ? [...(raw as unknown[])] : [];
  events.sort((a, b) => pyCompare(get(a, "time") ?? 0, get(b, "time") ?? 0));
  const rows = events.map((event) => {
    const details = get(event, "details") || {};
    const kept: Json = {};
    for (const key of EVENT_DETAILS) {
      if (Object.hasOwn(details, key)) kept[key] = (details as Json)[key];
    }
    return {
      type: get(event, "type"),
      clientId: get(event, "clientId"),
      error: get(event, "error"),
      details: kept,
    };
  });
  const counts = new Map<string, number>();
  for (const row of rows) {
    const key = `${pyStr(row.type)} ${row.error ? pyStr(row.error) : ""}`.trim();
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const sorted = [...counts].sort(([a], [b]) => compareCodePoints(a, b));
  return { counts: Object.fromEntries(sorted), events: rows };
}

/** `str()` of a JSON scalar as Python prints it in an f-string. */
function pyStr(value: unknown): string {
  if (value === null || value === undefined) return "None";
  if (value === true) return "True";
  if (value === false) return "False";
  return typeof value === "string" ? value : JSON.stringify(value);
}

/** The summary of one group: Playwright stats, else the last cargo `test result:` line. */
export function groupSummary(report: unknown, log: string | undefined): string {
  if (report === undefined) {
    if (log === undefined) return "no report";
    const results = log
      .split(/\r\n|\r|\n/)
      .filter((line) => line.startsWith("test result:"))
      .map((line) => line.trim());
    return results.at(-1) ?? "no report";
  }
  const stats = get(report, "stats") ?? {};
  const stat = (key: string) => pyStr(Object.hasOwn(stats, key) ? (stats as Json)[key] : 0);
  return (
    `passed ${stat("expected")}, failed ${stat("unexpected")}, ` +
    `flaky ${stat("flaky")}, skipped ${stat("skipped")}`
  );
}
