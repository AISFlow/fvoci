import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { t } from "@fvoci/i18n";
import { ProblemError } from "./api.ts";
import {
  clickOidcStart,
  OIDC_ERROR_CODES,
  type OidcStartDeps,
  oidcErrorMessage,
  oidcInviteStartForm,
  oidcLinkAction,
  oidcStartHref,
  readMfaFragment,
  startOidcInvite,
  startOidcLink,
  startWorkspaceSso,
  WORKSPACE_SSO_ACTION,
  workspaceSsoHref,
} from "./oidc.ts";

test("oidcErrorMessage maps each callback code to its catalog message", () => {
  for (const code of OIDC_ERROR_CODES) {
    const message = oidcErrorMessage(code);
    assert.equal(message, t(code));
    assert.notEqual(message, t("oidc_fallback"));
  }
  assert.equal(oidcErrorMessage("oidc_last_method"), "마지막 로그인 수단은 해제할 수 없습니다.");
});

test("oidcErrorMessage falls back for unknown codes and stays silent without one", () => {
  assert.equal(oidcErrorMessage("something_else"), t("oidc_fallback"));
  assert.equal(oidcErrorMessage("__proto__"), t("oidc_fallback"));
  assert.equal(oidcErrorMessage(null), null);
  assert.equal(oidcErrorMessage(undefined), null);
  assert.equal(oidcErrorMessage(""), null);
});

test("readMfaFragment returns the pending token from #mfa=", () => {
  assert.equal(readMfaFragment("#mfa=abc_DEF-123"), "abc_DEF-123");
  assert.equal(readMfaFragment("mfa=abc"), "abc");
  assert.equal(readMfaFragment("#other=1&mfa=tok%2Bx"), "tok+x");
});

test("readMfaFragment ignores empty or unrelated fragments", () => {
  assert.equal(readMfaFragment(""), null);
  assert.equal(readMfaFragment("#"), null);
  assert.equal(readMfaFragment("#mfa="), null);
  assert.equal(readMfaFragment("#section-2"), null);
});

test("oidcStartHref is the plain sign-in start", () => {
  assert.equal(oidcStartHref("google"), "/api/v1/auth/oidc/google/start");
  assert.equal(oidcStartHref("a/b"), "/api/v1/auth/oidc/a%2Fb/start");
  assert.equal(oidcLinkAction("a/b"), "/api/v1/auth/oidc/a%2Fb/link");
});

test("oidcInviteStartForm posts the invitation and consents as fields", () => {
  const form = oidcInviteStartForm("google", {
    token: "inv123",
    consents: [{ kind: "terms", version: 2 }],
  });
  // The action carries no query: nothing of the invitation is in the URL.
  assert.equal(form.action, "/api/v1/auth/oidc/google/start");
  assert.equal(form.fields.invitation, "inv123");
  assert.deepEqual(JSON.parse(form.fields.consents), [{ kind: "terms", version: 2 }]);
});

type Sent = { input: string; init: RequestInit };

function fakeStart(respond: () => Response | Promise<Response>) {
  const sent: Sent[] = [];
  const navigated: string[] = [];
  const deps: OidcStartDeps = {
    fetch: async (input, init) => {
      sent.push({ input, init });
      return respond();
    },
    navigate: (url) => void navigated.push(url),
  };
  return { deps, sent, navigated };
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const AUTHORIZE = "https://idp.example/authorize?client_id=c&state=s";

/**
 * The server refuses a POST start whose `Origin` is not the app's. A `fetch`
 * sends the page's origin only in mode `cors` (the default); any other mode
 * falls back to the referrer policy, and the app's `no-referrer` turns
 * `Origin` into `null`.
 */
function assertCorsMode(init: RequestInit) {
  assert.ok(init.mode === undefined || init.mode === "cors", `mode ${init.mode}`);
}

test("startOidcInvite posts the fields by fetch, then navigates to the provider", async () => {
  const fake = fakeStart(() => json(200, { authorizationUrl: AUTHORIZE }));
  await startOidcInvite(
    "google",
    { token: "inv 1&2", consents: [{ kind: "terms", version: 2 }] },
    fake.deps,
  );
  assert.equal(fake.sent.length, 1);
  const { input, init } = fake.sent[0]!;
  assert.equal(input, "/api/v1/auth/oidc/google/start");
  assert.equal(init.method, "POST");
  // Same-origin only: the state cookie must be stored, nothing is sent elsewhere.
  assert.equal(init.credentials, "same-origin");
  assertCorsMode(init);
  assert.ok(init.body instanceof URLSearchParams);
  const body = init.body as URLSearchParams;
  assert.deepEqual([...body.keys()], ["invitation", "consents"]);
  assert.equal(body.get("invitation"), "inv 1&2");
  assert.deepEqual(JSON.parse(body.get("consents") ?? ""), [{ kind: "terms", version: 2 }]);
  // A urlencoded body: the server accepts only that content type.
  assert.match(
    new Request("http://x/", { method: "POST", body }).headers.get("content-type") ?? "",
    /^application\/x-www-form-urlencoded/,
  );
  assert.deepEqual(fake.navigated, [AUTHORIZE]);
});

test("startOidcLink posts without a body and navigates", async () => {
  const fake = fakeStart(() => json(200, { authorizationUrl: AUTHORIZE }));
  await startOidcLink("a/b", fake.deps);
  assert.equal(fake.sent[0]!.input, "/api/v1/auth/oidc/a%2Fb/link");
  assert.equal(fake.sent[0]!.init.method, "POST");
  assert.equal(fake.sent[0]!.init.credentials, "same-origin");
  assertCorsMode(fake.sent[0]!.init);
  assert.equal(fake.sent[0]!.init.body, undefined);
  assert.deepEqual(fake.navigated, [AUTHORIZE]);
});

test("a refused or malformed start throws and never navigates", async () => {
  const refused = fakeStart(() => json(403, { code: "origin_mismatch", status: 403 }));
  await assert.rejects(
    startOidcInvite("google", { token: "t", consents: [] }, refused.deps),
    (err: unknown) =>
      err instanceof ProblemError && err.status === 403 && err.code === "origin_mismatch",
  );
  assert.deepEqual(refused.navigated, []);
  for (const body of [
    {},
    { authorizationUrl: 3 },
    { authorizationUrl: "javascript:alert(1)" },
    { authorizationUrl: "/relative" },
  ]) {
    const bad = fakeStart(() => json(200, body));
    await assert.rejects(
      startOidcLink("google", bad.deps),
      (err: unknown) => err instanceof ProblemError && err.status === 500,
      JSON.stringify(body),
    );
    assert.deepEqual(bad.navigated, [], JSON.stringify(body));
  }
  const html = fakeStart(() => new Response("<html>", { status: 502 }));
  await assert.rejects(
    startOidcLink("google", html.deps),
    (err: unknown) => err instanceof ProblemError && err.status === 502,
  );
});

test("the provider button click: pending while leaving, the problem on failure", async () => {
  const events: string[] = [];
  const ui = {
    setPending: (p: string | null) => void events.push(`pending:${p}`),
    setError: (m: string | null) => void events.push(`error:${m}`),
  };
  const ok = fakeStart(() => json(200, { authorizationUrl: AUTHORIZE }));
  await clickOidcStart(
    "google",
    () => startOidcInvite("google", { token: "t", consents: [] }, ok.deps),
    ui,
    "error.auth.invite",
  );
  // The page is leaving: the buttons stay disabled.
  assert.deepEqual(events, ["error:null", "pending:google"]);
  assert.deepEqual(ok.navigated, [AUTHORIZE]);

  events.length = 0;
  const refused = fakeStart(() => json(404, { code: "provider_not_configured", status: 404 }));
  await clickOidcStart(
    "kakao",
    () => startOidcInvite("kakao", { token: "t", consents: [] }, refused.deps),
    ui,
    "error.auth.invite",
  );
  assert.deepEqual(events, [
    "error:null",
    "pending:kakao",
    `error:${new ProblemError(404, "provider_not_configured").title}`,
    "pending:null",
  ]);
  assert.deepEqual(refused.navigated, []);

  events.length = 0;
  const offline = fakeStart(() => Promise.reject(new TypeError("Failed to fetch")));
  await clickOidcStart("google", () => startOidcLink("google", offline.deps), ui, "error.link");
  assert.deepEqual(events, [
    "error:null",
    "pending:google",
    `error:${t("error.network")}`,
    "pending:null",
  ]);
});

test("the SSO slug form navigates to the slug's start", () => {
  assert.equal(WORKSPACE_SSO_ACTION, "/api/v1/auth/sso");
  assert.equal(workspaceSsoHref("a&b c"), "/api/v1/auth/sso?slug=a%26b%20c");
  const navigated: string[] = [];
  const navigate = (url: string) => void navigated.push(url);
  assert.equal(startWorkspaceSso("acme-2", navigate), null);
  // Trimmed and NFKC-folded as the server reads it (fullwidth -> ASCII).
  assert.equal(startWorkspaceSso("  ａｃｍｅ\n", navigate), null);
  assert.deepEqual(navigated, ["/api/v1/auth/sso?slug=acme-2", "/api/v1/auth/sso?slug=acme"]);

  // By default the page itself goes there: `location.assign`, not a form.
  const assigned: string[] = [];
  const g = globalThis as { window?: unknown };
  const saved = g.window;
  g.window = { location: { assign: (url: string) => void assigned.push(url) } };
  try {
    assert.equal(startWorkspaceSso("acme"), null);
  } finally {
    g.window = saved;
  }
  assert.deepEqual(assigned, ["/api/v1/auth/sso?slug=acme"]);
});

test("the SSO slug form stays put on a slug the server would refuse", () => {
  const navigated: string[] = [];
  const navigate = (url: string) => void navigated.push(url);
  assert.equal(startWorkspaceSso("", navigate), "form.too_small");
  assert.equal(startWorkspaceSso("   ", navigate), "form.too_small");
  for (const slug of ["a", "Acme", "ac me", "acme/x", "a".repeat(33), "acme?slug=x", "워크"]) {
    assert.equal(startWorkspaceSso(slug, navigate), "form.invalid", slug);
  }
  assert.deepEqual(navigated, []);
  assert.equal(t("form.too_small"), "값을 입력해 주세요.");
  assert.equal(t("form.invalid"), "입력을 확인해 주세요.");
});

test("the login page's SSO form submits through startWorkspaceSso, never to the server", () => {
  const login = readFileSync(
    path.join(import.meta.dirname, "../features/auth/login.tsx"),
    "utf8",
  );
  const form = /function SsoSlugForm\(\)[\s\S]*?\n}\n/.exec(login)?.[0] ?? "";
  assert.match(form, /<form\b/);
  assert.match(form, /event\.preventDefault\(\);/);
  assert.match(form, /startWorkspaceSso\(/);
  assert.doesNotMatch(form, /\b(method|action|formAction)=/);
  // Nor does any other form on the login page.
  assert.doesNotMatch(login, /\b(method|action)=/);
});

test("the invite and account pages start OIDC by script, never by a native form", () => {
  for (const [file, start] of [
    ["../features/auth/invite.tsx", "startOidcInvite"],
    ["../features/settings/settings-account.tsx", "startOidcLink"],
  ] as const) {
    const page = readFileSync(path.join(import.meta.dirname, file), "utf8");
    // The provider button posts by fetch, then navigates by script.
    assert.match(page, new RegExp(`clickOidcStart\\([^;]*?\\b${start}\\(`), file);
    // No native submission on the page: a form post carries `Origin: null`
    // under the app's `no-referrer` (refused), and its redirect to the
    // provider is blocked by the CSP `form-action 'self'`.
    assert.doesNotMatch(page, /\b(method|action|formAction|formMethod)=/, file);
  }
});
