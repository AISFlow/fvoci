// Redaction of every log and evidence line of scripts/keycloak-oidc-e2e.sh.
//
// Patterns use Python `re` semantics on purpose: `\b` and `\s` are the
// Unicode word boundary and whitespace of the replaced helper, so a log line
// is redacted exactly as before. Each rule below documents its own scope.

// Python's `\s` for str patterns (JavaScript's lacks \x1c-\x1f and \x85, and
// adds \ufeff).
const S =
  "\\t\\n\\v\\f\\r\\x1c-\\x20\\x85\\xa0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
const WORD = "[\\p{L}\\p{N}_]";
// `\b` before / after a token that starts / ends with a word character.
const B = `(?<!${WORD})`;
const BE = `(?!${WORD})`;

// Terminal colour codes go first, raw or JSON-escaped (a coloured Playwright
// diff inside a JSON report), so `ESC[3mstateESC[0m=...` reads as `state=...`.
const ANSI = new RegExp(String.raw`(?:\x1b|\\\\?u001[bB])\[[0-9;]*m`, "gu");
// Names whose value is secret. code/state/nonce/session_state also name
// ordinary things, so for those a value that cannot be an OAuth value is kept:
// a lower_snake_case word ("origin_mismatch", "open"); for code also a short
// number (SMTP 550) or an errno name (ECONNREFUSED); for state a capitalised
// word (Open). Every other value is redacted. (A lowercase-word MFA recovery
// code would be kept under `code`; this check never prints one.)
const AMBIGUOUS_KEYS = "code|state|nonce|session_state";
const ALWAYS_KEYS =
  "code_id|session_code|client_data|tab_id|auth_session_[a-z_]+|userSessionId" +
  "|sessionId|session_id|code_challenge|code_verifier|access_token|refresh_token" +
  "|id_token|id_token_hint|invitation|mfa";
const KEYS = `${AMBIGUOUS_KEYS}|${ALWAYS_KEYS}`;
const PLAIN_WORD = "[a-z][a-z_]{0,63}";
const KEPT_CODE = `(?:${PLAIN_WORD}|[0-9]{1,5}|E[A-Z]{2,31})`;
const KEPT_STATE = `(?:${PLAIN_WORD}|[A-Z][a-z]{1,31})`;
const KEPT_BODY: Record<string, string> = {
  code: KEPT_CODE,
  state: KEPT_STATE,
  nonce: PLAIN_WORD,
  session_state: PLAIN_WORD,
};
const KEPT = new Map(
  Object.entries(KEPT_BODY).map(([key, body]) => [key, new RegExp(`^${body}$`, "u")]),
);

// A quoted value after `key=`, `key: ` (Rust Debug), `"key": ` (JSON) or
// `\"key\":` (JSON inside a JSON string), optionally inside `Some(...)`. The
// value is lexed as a string of its own quoting: `\x` escapes in a plain
// string; in an escaped string (`\"...\"`) the doubled escapes `\\\"`,
// `\\\\` and `\\x`.
const QUOTED = new RegExp(
  `(?<pre>(?:${B}(?<k1>${KEYS})(?:=|:[ \\t]*)|(?<jq>\\\\?)"(?<k3>${KEYS})\\k<jq>"[ \\t]*:[ \\t]*)(?:Some\\()?)` +
    String.raw`(?:(?<eo>\\")(?<ev>(?:[^"\\]|\\\\\\["\\]|\\\\[^"\\])*)\\"|(?<po>")(?<pv>(?:[^"\\]|\\[^\n])*)")`,
  "gu",
);
// Fail closed: any other `key=` / `key:` / `"key":` form (spaces, single
// quotes, String("..."), unquoted Some(...), deeper JSON escaping) whose value
// is neither redacted nor a kept word in its own quotes cuts the line there.
const SENSITIVE = new RegExp(
  `(?:\\\\*"(?<jk>${KEYS})\\\\*"|${B}(?<k>${KEYS})${BE})[ \\t]*[=:][ \\t]*`,
  "gu",
);
const REDACTED = "<redacted[a-z-]*>";

function settledValue(key: string): RegExp {
  const body = KEPT_BODY[key] ?? "(?!)";
  return new RegExp(
    `(?:Some\\()?(?:(?<q>\\\\*["'])(?:${REDACTED}|${body})\\k<q>` +
      `|(?:${REDACTED}|${body})(?=$|[${S}"'\\\\&,;)}\\]]))`,
    "uy",
  );
}

// key=value in URLs and logs (unquoted).
const URL_PARAMS = new RegExp(
  `${B}(?<key>${KEYS})=(?!<redacted>|Some\\(|\\\\?")(?<value>(?:[^&${S}"'<>\\\\;,)]|\\\\(?!"))+)`,
  "gu",
);
const COOKIES = new RegExp(
  `${B}(fvoci_session|fvoci_oidc_state|KEYCLOAK_[A-Z_]+|AUTH_SESSION_ID[A-Z_]*|KC_RESTART` +
    `|KC_AUTH_SESSION_HASH|KC_STATE_CHECKER)=[^;${S}"']+`,
  "gu",
);
const JWT = /eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*/gu;
const INVITE = /\/invite\/[A-Za-z0-9_-]{16,}/gu;
const POSTGRES = new RegExp(`postgres(?:ql)?://[^${S}]+`, "gu");
const BEARER = new RegExp(`${B}Bearer[${S}]+[^${S}]+`, "gu");
const BASIC = new RegExp(`${B}Basic[${S}]+(?<token>[A-Za-z0-9+/]{7,}={0,2})`, "gu");
// A Basic credential, not prose ("Basic authentication"): base64 of 8+
// characters with a digit, "+", "/", padding or an uppercase letter past the
// first character.
const CREDENTIAL_SHAPE = /[0-9+/=]|[^\n][A-Z]/u;

type Groups = Record<string, string | undefined>;

function keeps(key: string | undefined, value: string | undefined): boolean {
  const kept = key === undefined ? undefined : KEPT.get(key);
  return kept !== undefined && kept.test(value ?? "");
}

const settledCache = new Map<string, RegExp>();

/** Cuts the line at a sensitive value no rule before it redacted or kept. */
function failClosed(line: string): string {
  for (const match of line.matchAll(SENSITIVE)) {
    const groups = match.groups as Groups;
    const key = (groups.jk ?? groups.k) as string;
    let settled = settledCache.get(key);
    if (settled === undefined) {
      settled = settledValue(key);
      settledCache.set(key, settled);
    }
    const end = match.index + match[0].length;
    settled.lastIndex = end;
    if (!settled.test(line)) {
      return `${line.slice(0, end)}<redacted-rest>${line.endsWith("\n") ? "\n" : ""}`;
    }
  }
  return line;
}

/** One line (with its newline, if any) without secrets or OAuth values. */
export function redactLine(input: string, secrets: readonly string[]): string {
  // Known secrets go first (raw or JSON-escaped), before any rule can change
  // part of one, and again once colour codes are gone.
  let line = replaceSecrets(input, secrets).replace(ANSI, "");
  line = replaceSecrets(line, secrets);
  line = line.replace(QUOTED, (whole: string, ...rest: unknown[]) => {
    const groups = rest.at(-1) as Groups;
    if (keeps(groups.k1 ?? groups.k3, groups.ev ?? groups.pv)) return whole;
    const escaped = groups.eo !== undefined;
    const quote = escaped ? '\\"' : '"';
    return `${groups.pre ?? ""}${quote}<redacted>${quote}`;
  });
  line = line.replace(URL_PARAMS, (whole: string, ...rest: unknown[]) => {
    const groups = rest.at(-1) as Groups;
    return keeps(groups.key, groups.value) ? whole : `${groups.key ?? ""}=<redacted>`;
  });
  line = failClosed(line);
  line = line.replace(COOKIES, "$1=<redacted>");
  line = line.replace(JWT, "<redacted-jwt>");
  line = line.replace(INVITE, "/invite/<redacted>");
  line = line.replace(POSTGRES, "postgres://<redacted>");
  line = line.replace(BEARER, "Bearer <redacted>");
  return line.replace(BASIC, (whole: string, ...rest: unknown[]) => {
    const token = (rest.at(-1) as Groups).token ?? "";
    return token.length >= 8 && CREDENTIAL_SHAPE.test(token) ? "Basic <redacted>" : whole;
  });
}

// --- Known secrets of this process -------------------------------------------
//
// Every per-run secret the helper reads (environment, configs) or obtains
// (the admin access token) is registered here, and every byte the helper
// writes to stdout or stderr goes through `scrubText`. A value that cannot be
// scrubbed reliably is refused instead of being left out: one shorter than
// MIN_SECRET_LENGTH would also match ordinary text, and one with a line break
// or control character would be split across separately written lines (or
// changed by the colour-code removal) and escape the per-line scrub.
//
// A refusal names the secret by a fixed field name and numeric index only,
// never by a value or key taken from the input: nothing may reach the output
// that the registry has not seen.

/** An error whose message is written out (scrubbed): fixed text, numeric indexes and registered values only. */
export class HelperError extends Error {}

/**
 * The error's code (ENOENT, ConnectionRefused, ...) when it is an identifier,
 * else its name (TimeoutError, ...); never its message, which may quote input.
 */
export function errorName(error: unknown): string {
  if (!(error instanceof Error)) return typeof error;
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" && /^[A-Za-z][A-Za-z0-9_]{0,63}$/.test(code) ? code : error.name;
}

export const MIN_SECRET_LENGTH = 8;
const known = new Set<string>();

const length = (text: string) => Array.from(text).length;

const UNSPLITTABLE = /[\p{Cc}\u2028\u2029]/u;

/** Why a secret cannot be registered, or undefined when it can. */
function refusal(value: string, what: string): string | undefined {
  if (length(value) < MIN_SECRET_LENGTH)
    return `${what} is shorter than ${String(MIN_SECRET_LENGTH)} characters`;
  if (UNSPLITTABLE.test(value)) return `${what} contains a line break or control character`;
  return undefined;
}

/** Registers a secret; `what` (fixed text) names it in the refusal, never the value. */
export function addSecret(value: string, what: string): void {
  const refused = refusal(value, what);
  if (refused !== undefined) throw new HelperError(refused);
  known.add(value);
}

/** The registered secrets, longest first (a secret inside another goes after it). */
export function knownSecrets(): string[] {
  return Array.from(known).sort((a, b) => length(b) - length(a));
}

/** Forgets every registered secret (tests only). */
export function forgetSecrets(): void {
  known.clear();
}

const jsonEscaped = (text: string) => JSON.stringify(text).slice(1, -1);
const asciiEscaped = (text: string) =>
  jsonEscaped(text).replace(
    new RegExp("[\\u007f-\\uffff]", "g"),
    (char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );

/** The text with each of `secrets` (longest first), raw or JSON-escaped, replaced. */
function replaceSecrets(text: string, secrets: readonly string[]): string {
  let out = text;
  for (const secret of secrets) {
    for (const form of new Set([secret, jsonEscaped(secret), asciiEscaped(secret)])) {
      out = out.replaceAll(form, "<redacted-secret>");
    }
  }
  return out;
}

/** The text with every registered secret, raw or JSON-escaped, replaced. */
export function scrubText(text: string): string {
  return replaceSecrets(text, knownSecrets());
}

/** Registers every secret of a spec config: client secrets, passwords, the admin password. */
export function registerConfigSecrets(config: unknown): void {
  const record = (config ?? {}) as {
    secrets?: unknown;
    users?: unknown;
    admin?: unknown;
    fvoci?: unknown;
  };
  const problems: string[] = [];
  const object = (value: unknown, problem: string): object => {
    if (value !== null && typeof value === "object") return value;
    problems.push(problem);
    return {};
  };
  const users = object(record.users, "config has no users");
  const given = record.secrets ?? [];
  if (!Array.isArray(given)) problems.push("config secrets is not a list");
  const secrets: unknown[] = Array.isArray(given) ? given : [];
  const fvoci = object(record.fvoci ?? {}, "config fvoci is not an object");
  const member = (value: unknown, key: string): unknown =>
    value !== null && typeof value === "object"
      ? (value as Record<string, unknown>)[key]
      : undefined;
  // Users and FVOCI members are named by position (JavaScript key order),
  // never by key: their keys are input.
  const listed: [string, unknown][] = [
    ...secrets.map((value, index): [string, unknown] => [`config secrets.${String(index)}`, value]),
    ...Object.values(users).map((user, index): [string, unknown] => [
      `config users.${String(index)}.password`,
      member(user, "password"),
    ]),
    ["config admin.password", member(record.admin, "password")],
    ...Object.values(fvoci).map((value, index): [string, unknown] => [
      `config fvoci.${String(index)}`,
      value,
    ]),
  ];
  // Every usable secret is registered before the first refusal is raised, so
  // a later error path still scrubs them.
  for (const [what, value] of listed) {
    if (value === undefined) continue;
    const refused = typeof value === "string" ? refusal(value, what) : `${what} is not a string`;
    if (refused === undefined) known.add(value as string);
    else problems.push(refused);
  }
  if (problems.length > 0) throw new HelperError(problems[0]);
}

/**
 * Splits a decoded stream at "\n" (the newline of Python's POSIX stdin; "\r"
 * stays in the line) and keeps the unfinished tail for the next chunk.
 */
export class LineSplitter {
  private tail = "";

  push(chunk: string): string[] {
    const parts = (this.tail + chunk).split("\n");
    this.tail = parts.pop() ?? "";
    return parts.map((part) => `${part}\n`);
  }

  end(): string[] {
    const rest = this.tail;
    this.tail = "";
    return rest === "" ? [] : [rest];
  }
}

/** (input, expected output) pairs run by `selftest` before every run. */
export const REDACTION_CASES: readonly (readonly [string, string])[] = [
  ['"state": "abc&X"', '"state": "<redacted>"'],
  ['state="a\\\\bX"', 'state="<redacted>"'],
  ["/cb?state=\\u001b[7mXsecretX", "/cb?state=<redacted>"],
  [
    '{"message":"url /cb?state=\\u001b[7mQw9_Zz\\u001b[27m&x=1"}',
    '{"message":"url /cb?state=<redacted>&x=1"}',
  ],
  ['"code": "x\\"y-SECRET"', '"code": "<redacted>"'],
  ['state=Some("SECRETX")', 'state=Some("<redacted>")'],
  [
    'Foo { nonce: "SECRETN", code: "origin_mismatch" }',
    'Foo { nonce: "<redacted>", code: "origin_mismatch" }',
  ],
  [
    'code=550 "code": "ECONNREFUSED" state=Open state="open"',
    'code=550 "code": "ECONNREFUSED" state=Open state="open"',
  ],
  [
    'state="SECRETX" nonce=Open code=SECRETX session_state=Open',
    'state="<redacted>" nonce=<redacted> code=<redacted> session_state=<redacted>',
  ],
  [
    '\x1b[3mstate\x1b[0m\x1b[2m=\x1b[0m"Ab3-xY" \x1b[3mreason\x1b[0m\x1b[2m=\x1b[0m"oidc_not_linked"',
    'state="<redacted>" reason="oidc_not_linked"',
  ],
  [
    '{"msg":"{\\"state\\":\\"Qw9_Zz\\",\\"code\\":\\"origin_mismatch\\",\\"access_token\\":\\"t0k\\"}"}',
    '{"msg":"{\\"state\\":\\"<redacted>\\",\\"code\\":\\"origin_mismatch\\",\\"access_token\\":\\"<redacted>\\"}"}',
  ],
  ['{\\"code\\":\\"a\\\\\\"b-SECRET\\"}', '{\\"code\\":\\"<redacted>\\"}'],
  ['"state": "unterminated SECRET', '"state": <redacted-rest>'],
  [
    '"url": "\\"http://h/cb?code=a1b-2c\\" next"',
    '"url": "\\"http://h/cb?code=<redacted>\\" next"',
  ],
  ['"state": String("X") tail', '"state": <redacted-rest>'],
  ["state: 'X' tail", "state: <redacted-rest>"],
  ["state='X' tail", "state=<redacted-rest>"],
  ['state = "X" tail', "state = <redacted-rest>"],
  ["state=Some(X) tail", "state=<redacted-rest>"],
  ['\\\\\\"state\\\\\\":\\\\\\"X\\\\\\"} tail', '\\\\\\"state\\\\\\":<redacted-rest>'],
  [
    'state=Some(Open) state = "open" "code": 403, \\"code\\":\\"origin_mismatch\\"',
    'state=Some(Open) state = "open" "code": 403, \\"code\\":\\"origin_mismatch\\"',
  ],
  ["mail failed code=smtp_timeout; next", "mail failed code=smtp_timeout; next"],
  [
    "GET /cb?code=9f2c.aa-11&state=abcDEF123&iss=x",
    "GET /cb?code=<redacted>&state=<redacted>&iss=x",
  ],
  [
    'type="X", code_id="abc-123", auth_session_parent_id="p1", userSessionId="u1", code="XyZ.123"',
    'type="X", code_id="<redacted>", auth_session_parent_id="<redacted>", userSessionId="<redacted>", code="<redacted>"',
  ],
  [
    "Basic authentication is off; Basic Authentication",
    "Basic authentication is off; Basic Authentication",
  ],
  [
    "Authorization: Basic dXNlcjpwYXNz; Bearer abc.def",
    "Authorization: Basic <redacted>; Bearer <redacted>",
  ],
  [
    "fvoci_session=abc; /invite/abcdefghijklmnopqrst postgres://u:p@h/db eyJhbGciOiJ9.eyJzdWIiOjF9.sig",
    "fvoci_session=<redacted>; /invite/<redacted> postgres://<redacted> <redacted-jwt>",
  ],
];

/** Failed cases as [index, got]; empty when every case holds. */
export function selftestFailures(): [number, string][] {
  const failed: [number, string][] = [];
  REDACTION_CASES.forEach(([given, want], index) => {
    const got = redactLine(given, []);
    if (got !== want) failed.push([index, got]);
  });
  return failed;
}
