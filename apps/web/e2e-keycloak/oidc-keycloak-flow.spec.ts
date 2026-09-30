// Real Keycloak (official image, start-dev) as an instance OIDC provider for
// the release FVOCI server and the built web UI, in a real Chromium. Run by
// scripts/keycloak-oidc-e2e.sh; skipped unless FVOCI_KC_E2E_CONFIG names that
// runner's per-run config. FVOCI_KC_E2E_MODE picks the group: each group has
// its own server and database (the OIDC rate limit is 30 starts/callbacks per
// IP per 5 minutes and in memory, so one server cannot take every flow).
//
// No value of a password, client secret, code, state, nonce, token or cookie
// is printed, recorded, or passed to an assertion that would print it.
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import {
  expect,
  request as playwrightRequest,
  test,
  type Browser,
  type BrowserContext,
  type Cookie,
  type Page,
  type Request,
  type Response as PlaywrightResponse,
} from "@playwright/test";
import { login, logout, watchCspViolations } from "../e2e/helpers";

type KcUserKey = "alice" | "bob" | "carol" | "mallory" | "erin" | "tina";
type KcUser = { username: string; password: string; email: string };
type KcConfig = {
  issuer: string;
  realm: string;
  clientId: string;
  label: string;
  keycloakOrigin: string;
  admin: { username: string; password: string };
  users: Record<KcUserKey, KcUser>;
  /** Passwords of the FVOCI accounts this spec creates (the runner's redactor knows them). */
  fvoci: { ownerPassword: string; memberPassword: string };
};

const configPath = process.env.FVOCI_KC_E2E_CONFIG ?? "";
const mode = process.env.FVOCI_KC_E2E_MODE ?? "";
const outDir = process.env.FVOCI_KC_E2E_OUT ?? "";
const kc: KcConfig | null =
  configPath && existsSync(configPath)
    ? (JSON.parse(readFileSync(configPath, "utf8")) as KcConfig)
    : null;

const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:1";
const fvociOrigin = new URL(baseURL).origin;
const CALLBACK_PATH = "/api/v1/auth/oidc/generic/callback";
const redirectUri = `${fvociOrigin}${CALLBACK_PATH}`;

const WORKSPACE = { slug: "kc-e2e", name: "Keycloak E2E 워크스페이스" };
const OWNER = {
  email: "kc-owner@example.com",
  password: kc?.fvoci.ownerPassword ?? "",
  familyName: "김",
  givenName: "소유자",
};
const PAT = {
  email: "kc-pat@example.com",
  password: kc?.fvoci.memberPassword ?? "",
  familyName: "박",
  givenName: "비번",
};

const MSG = {
  notLinked: "연결된 소셜 계정이 없습니다. 이메일로 로그인해 주세요.",
  stateMismatch: "로그인 요청이 만료되었습니다. 다시 시도해 주세요.",
  providerError: "소셜 로그인을 완료하지 못했습니다. 다시 시도해 주세요.",
  alreadyLinked: "이미 다른 계정에 연결된 소셜 계정입니다.",
  linked: "소셜 계정이 연결되었습니다.",
};

const observations: Record<string, unknown> = {};
function observe(key: string, value: unknown): void {
  observations[key] = value;
}

function config(): KcConfig {
  if (!kc) throw new Error("FVOCI_KC_E2E_CONFIG is not set");
  return kc;
}

// ---------------------------------------------------------------------------
// Keycloak admin API (master realm admin of this run's container)

async function kcAdmin(path: string, init: RequestInit = {}): Promise<Response> {
  const c = config();
  const tokenResponse = await fetch(
    `${c.keycloakOrigin}/realms/master/protocol/openid-connect/token`,
    {
      method: "POST",
      body: new URLSearchParams({
        grant_type: "password",
        client_id: "admin-cli",
        username: c.admin.username,
        password: c.admin.password,
      }),
    },
  );
  if (!tokenResponse.ok) throw new Error(`Keycloak admin token: HTTP ${tokenResponse.status}`);
  const { access_token: token } = (await tokenResponse.json()) as { access_token: string };
  return fetch(`${c.keycloakOrigin}/admin/realms/${c.realm}${path}`, {
    ...init,
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
  });
}

type KcClient = Record<string, unknown> & {
  id: string;
  redirectUris: string[];
  webOrigins: string[];
};

async function kcClient(): Promise<KcClient> {
  const response = await kcAdmin(`/clients?clientId=${encodeURIComponent(config().clientId)}`);
  expect(response.status).toBe(200);
  const clients = (await response.json()) as KcClient[];
  expect(clients).toHaveLength(1);
  return clients[0];
}

/** Registers the one redirect URI this server generates (its port is chosen at start). */
async function registerRedirectUri(): Promise<void> {
  const client = await kcClient();
  const put = await kcAdmin(`/clients/${client.id}`, {
    method: "PUT",
    body: JSON.stringify({ ...client, redirectUris: [redirectUri], webOrigins: [] }),
  });
  expect(put.status).toBe(204);
  const after = await kcClient();
  expect(after.redirectUris).toEqual([redirectUri]);
  expect(after.webOrigins).toEqual([]);
  const attributes = after.attributes as Record<string, string>;
  const settings = {
    publicClient: after.publicClient,
    clientAuthenticatorType: after.clientAuthenticatorType,
    standardFlowEnabled: after.standardFlowEnabled,
    implicitFlowEnabled: after.implicitFlowEnabled,
    directAccessGrantsEnabled: after.directAccessGrantsEnabled,
    serviceAccountsEnabled: after.serviceAccountsEnabled,
    pkceMethod: attributes["pkce.code.challenge.method"],
    redirectUris: after.redirectUris,
    webOrigins: after.webOrigins,
    defaultClientScopes: after.defaultClientScopes,
    optionalClientScopes: after.optionalClientScopes,
  };
  expect(settings).toMatchObject({
    publicClient: false,
    clientAuthenticatorType: "client-secret",
    standardFlowEnabled: true,
    implicitFlowEnabled: false,
    directAccessGrantsEnabled: false,
    serviceAccountsEnabled: false,
    pkceMethod: "S256",
  });
  observe("keycloakClientAfterRegistration", settings);

  // Behaviour, not only the stored settings: without a PKCE challenge, or
  // for the implicit flow, Keycloak refuses to start.
  const refusals: Record<string, unknown> = {};
  for (const [name, responseType, challenge] of [
    ["codeWithoutPkce", "code", false],
    ["implicit", "id_token token", true],
  ] as const) {
    const probe = new URL(`${config().issuer}/protocol/openid-connect/auth`);
    const params: Record<string, string> = {
      response_type: responseType,
      client_id: config().clientId,
      redirect_uri: redirectUri,
      scope: "openid",
      state: "probe",
      nonce: "probe",
    };
    if (challenge) {
      params.code_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
      params.code_challenge_method = "S256";
    }
    for (const [key, value] of Object.entries(params)) probe.searchParams.set(key, value);
    const response = await fetch(probe, { redirect: "manual" });
    const location = new URL(response.headers.get("location") ?? "", redirectUri);
    const answer = new URLSearchParams(location.hash.slice(1) || location.search.slice(1));
    expect(response.status).toBe(302);
    expect(`${location.origin}${location.pathname}`).toBe(redirectUri);
    expect(answer.has("code")).toBe(false);
    expect(answer.get("error")).toBeTruthy();
    refusals[name] = {
      error: answer.get("error"),
      error_description: answer.get("error_description"),
    };
  }
  observe("keycloakAuthorizationRefusals", refusals);
}

async function kcUserId(username: string): Promise<string> {
  const response = await kcAdmin(`/users?exact=true&username=${encodeURIComponent(username)}`);
  const users = (await response.json()) as Array<{ id: string }>;
  expect(users).toHaveLength(1);
  return users[0].id;
}

async function kcSessionCount(username: string): Promise<number> {
  const response = await kcAdmin(`/users/${await kcUserId(username)}/sessions`);
  expect(response.status).toBe(200);
  return ((await response.json()) as unknown[]).length;
}

type Discovery = {
  issuer: string;
  authorization_endpoint: string;
  token_endpoint: string;
  jwks_uri: string;
  token_endpoint_auth_methods_supported?: string[];
  code_challenge_methods_supported?: string[];
  authorization_response_iss_parameter_supported?: boolean;
};

async function discovery(): Promise<Discovery> {
  const response = await fetch(`${config().issuer}/.well-known/openid-configuration`);
  expect(response.status).toBe(200);
  return (await response.json()) as Discovery;
}

// ---------------------------------------------------------------------------
// This group's database (superuser through the test container: counts only)

function sql(query: string): string {
  const container = process.env.FVOCI_TEST_PG_CONTAINER ?? "";
  const adminUrl = process.env.FVOCI_E2E_ADMIN_DATABASE_URL ?? "";
  if (!container || !adminUrl) throw new Error("the web e2e harness database is not available");
  const database = new URL(adminUrl).pathname.slice(1);
  return execFileSync(
    "docker",
    [
      "exec",
      "-i",
      container,
      "psql",
      "-U",
      "postgres",
      "-d",
      database,
      "-v",
      "ON_ERROR_STOP=1",
      "-tA",
      "-c",
      query,
    ],
    { encoding: "utf8" },
  ).trim();
}

function count(query: string): number {
  return Number(sql(query));
}

const userCount = () => count("SELECT count(*) FROM fvoci.users");
const stateCount = () => count("SELECT count(*) FROM fvoci.oidc_states");

type LinkRow = {
  user: string;
  provider: string;
  subject: string;
  issuer: string | null;
  email: string | null;
};

function links(): LinkRow[] {
  return JSON.parse(
    sql(
      `SELECT coalesce(json_agg(json_build_object('user', u.email, 'provider', l.provider,
         'subject', l.provider_user_id, 'issuer', l.issuer, 'email', l.email) ORDER BY u.email), '[]')
       FROM fvoci.identity_links l JOIN fvoci.users u ON u.id = l.user_id`,
    ),
  ) as LinkRow[];
}

function linksOf(email: string): LinkRow[] {
  return links().filter((l) => l.user === email);
}

function membershipRole(email: string): string {
  return sql(
    `SELECT m.role FROM fvoci.memberships m JOIN fvoci.workspaces w ON w.id = m.workspace_id
     JOIN fvoci.users u ON u.id = m.user_id WHERE w.slug = '${WORKSPACE.slug}' AND u.email = '${email}'`,
  );
}

function liveSessions(email: string): number {
  return count(
    `SELECT count(*) FROM fvoci.sessions s JOIN fvoci.users u ON u.id = s.user_id
     WHERE u.email = '${email}' AND s.revoked_at IS NULL AND s.expires_at > now()`,
  );
}

// ---------------------------------------------------------------------------
// Browser helpers

type Me = { userId: string; email: string; hasPassword: boolean };

async function me(page: Page): Promise<Me | null> {
  const response = await page.request.get("/api/v1/auth/me");
  if (response.status() === 401) return null;
  expect(response.status()).toBe(200);
  return (await response.json()) as Me;
}

async function workspaceSlugs(page: Page): Promise<string[]> {
  const response = await page.request.get("/api/v1/me/workspaces");
  expect(response.status()).toBe(200);
  const body = (await response.json()) as { items: Array<{ slug: string; kind: string }> };
  return body.items.filter((w) => w.kind === "team").map((w) => w.slug);
}

/** Vue's mount marker, observed on actual auth routes without recording tokens. */
async function expectVueAuthRoute(page: Page, route: string): Promise<void> {
  await expect(page.locator("#root.isolate[data-v-app]")).toHaveCount(1);
  const pathname = new URL(page.url()).pathname;
  expect(
    route === "/invite/:token" ? pathname.startsWith("/invite/") : pathname === route,
    `the mounted Vue auth route is ${route}`,
  ).toBe(true);
  observe(`vueRoute:${route}`, { mounted: true, root: "#root.isolate[data-v-app]" });
}

async function openInvitation(page: Page, token: string): Promise<void> {
  await page.goto(`/invite/${token}`);
  await expectVueAuthRoute(page, "/invite/:token");
}

async function setupOwner(page: Page): Promise<void> {
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await expectVueAuthRoute(page, "/setup");
  await page.getByLabel("성").fill(OWNER.familyName);
  await page.getByLabel("이름", { exact: true }).fill(OWNER.givenName);
  await page.getByLabel("이메일").fill(OWNER.email);
  await page.getByLabel("비밀번호").fill(OWNER.password);
  await page.getByLabel("워크스페이스 이름").fill(WORKSPACE.name);
  await page.getByLabel("주소(영문)").fill(WORKSPACE.slug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
}

/** Owner's members settings: creates an invitation and returns its token. */
async function inviteViaUi(page: Page, email: string): Promise<string> {
  await page.goto(`/w/${WORKSPACE.slug}/settings`);
  await page
    .locator("summary")
    .filter({ hasText: /^멤버$/ })
    .click();
  await page.getByLabel("초대할 이메일").fill(email);
  await page.getByRole("button", { name: "초대", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "초대를 만들었습니다" })).toBeVisible();
  const href = await page.getByRole("link").filter({ hasText: "/invite/" }).getAttribute("href");
  const token = href?.split("/invite/")[1] ?? "";
  expect(token.length > 0, "the invite link carries a token").toBe(true);
  return token;
}

async function newPage(
  browser: Browser,
): Promise<{ context: BrowserContext; page: Page; csp: string[] }> {
  const context = await browser.newContext({ baseURL });
  const page = await context.newPage();
  return { context, page, csp: watchCspViolations(page) };
}

/**
 * One trip through Keycloak. Created before the click that leaves FVOCI so
 * the authorization request and the callback are both observed, including
 * when Keycloak answers from its own session with a redirect only.
 */
function keycloakLeg(page: Page) {
  const authorizationEndpoint = `${config().issuer}/protocol/openid-connect/auth`;
  const authorization = page.waitForRequest(
    (r) => r.url().startsWith(`${authorizationEndpoint}?`),
    {
      timeout: 30_000,
    },
  );
  const callback = page.waitForRequest((r) => r.url().startsWith(`${redirectUri}?`), {
    timeout: 60_000,
  });
  authorization.catch(() => undefined);
  callback.catch(() => undefined);
  return {
    authorization,
    callback,
    /** Waits for Keycloak's sign-in form or its session redirect; fills the form for `user`. */
    async pass(user: KcUser | null): Promise<"form" | "sso"> {
      await authorization;
      const username = page.locator("#username");
      const formShown = await Promise.race([
        username.waitFor({ state: "visible", timeout: 30_000 }).then(
          () => true,
          () => false,
        ),
        callback.then(
          () => false,
          () => false,
        ),
      ]);
      if (!formShown) return "sso";
      if (!user) throw new Error("Keycloak asked for credentials");
      await username.fill(user.username);
      await page.locator("#password").fill(user.password);
      await page.locator("#kc-login").click();
      return "form";
    },
  };
}

/** Status and redirect target (path and query) of a callback request. */
async function callbackResult(request: Request): Promise<{ status: number; location: string }> {
  const response = await request.response();
  if (!response) throw new Error("the callback got no response");
  const header = response.headers()["location"];
  if (!header) return { status: response.status(), location: "" };
  const target = new URL(header, fvociOrigin);
  expect(target.origin).toBe(fvociOrigin);
  return { status: response.status(), location: `${target.pathname}${target.search}` };
}

async function visitCallback(
  page: Page,
  url: string,
): Promise<{ status: number; location: string }> {
  const request = page.waitForRequest((r) => r.url().startsWith(`${redirectUri}?`), {
    timeout: 30_000,
  });
  await page.goto(url);
  return callbackResult(await request);
}

/**
 * Stops the browser at the FVOCI callback (DevTools Fetch interception, which
 * also sees the redirect hop Keycloak answers with; Playwright's route does
 * not) and answers it locally, so the code and state stay unused.
 */
async function holdCallback(
  page: Page,
): Promise<{ url: Promise<string>; release: () => Promise<void> }> {
  const cdp = await page.context().newCDPSession(page);
  let held = false;
  let resolveUrl: (url: string) => void = () => undefined;
  const url = new Promise<string>((resolve) => {
    resolveUrl = resolve;
  });
  cdp.on("Fetch.requestPaused", (event) => {
    if (held) {
      void cdp.send("Fetch.continueRequest", { requestId: event.requestId }).catch(() => undefined);
      return;
    }
    held = true;
    resolveUrl(event.request.url);
    void cdp
      .send("Fetch.fulfillRequest", {
        requestId: event.requestId,
        responseCode: 200,
        responseHeaders: [{ name: "Content-Type", value: "text/plain; charset=utf-8" }],
        body: Buffer.from("callback held by the test").toString("base64"),
      })
      .catch(() => undefined);
  });
  await cdp.send("Fetch.enable", {
    patterns: [{ urlPattern: `${redirectUri}*`, requestStage: "Request" }],
  });
  return {
    url,
    release: async () => {
      await cdp.send("Fetch.disable");
      await cdp.detach();
    },
  };
}

async function clickProvider(page: Page): Promise<void> {
  await page.getByRole("link", { name: config().label, exact: true }).click();
}

async function openLogin(page: Page): Promise<void> {
  await page.goto("/login");
  await expect(page.getByRole("link", { name: config().label, exact: true })).toBeVisible();
  await expectVueAuthRoute(page, "/login");
}

async function openAccountSettings(page: Page): Promise<void> {
  await page.goto("/settings/account");
  await expect(page.getByText("연결된 소셜 계정")).toBeVisible();
  await expectVueAuthRoute(page, "/settings/account");
}

function linkButton(page: Page) {
  return page.getByRole("button", { name: "연결", exact: true });
}

/** Signs in with the provider from the login page; returns the user info. */
async function providerSignIn(
  page: Page,
  user: KcUser | null,
): Promise<{ via: "form" | "sso"; me: Me }> {
  await openLogin(page);
  const leg = keycloakLeg(page);
  await clickProvider(page);
  const via = await leg.pass(user);
  expect(await callbackResult(await leg.callback)).toEqual({ status: 302, location: "/" });
  await page.waitForURL((u) => u.origin === fvociOrigin && u.pathname === "/");
  const current = await me(page);
  expect(current).not.toBeNull();
  return { via, me: current as Me };
}

async function problemCode(response: PlaywrightResponse): Promise<string | null> {
  try {
    return ((await response.json()) as { code?: string }).code ?? null;
  } catch {
    return null;
  }
}

type SentPost = {
  origin?: string;
  sessionCookieSent?: boolean;
  status?: number;
  setsStateCookie?: boolean;
  /** Problem `code` of the answer (read before the browser's CORS check). */
  code?: string | null;
};

/**
 * The browser's own record (DevTools network events) of POSTs to `urls`: the
 * Origin it sent, whether the session cookie went along, the status and the
 * problem code that came back, also for answers the page may not read (the
 * body is taken at the Fetch domain's response stage, before CORS applies).
 */
async function watchPosts(page: Page, urls: readonly string[]) {
  const cdp = await page.context().newCDPSession(page);
  const urlById = new Map<string, string>();
  const byId = new Map<string, SentPost>();
  const order: string[] = [];
  const entry = (id: string): SentPost => {
    const found = byId.get(id) ?? {};
    byId.set(id, found);
    return found;
  };
  cdp.on("Network.requestWillBeSent", (event) => {
    if (event.request.method === "POST" && urls.includes(event.request.url)) {
      urlById.set(event.requestId, event.request.url);
      order.push(event.requestId);
    }
  });
  cdp.on("Network.requestWillBeSentExtraInfo", (event) => {
    const sent = entry(event.requestId);
    sent.origin = event.headers["Origin"] ?? event.headers["origin"];
    sent.sessionCookieSent = event.associatedCookies.some(
      (c) => c.cookie.name === "fvoci_session" && c.blockedReasons.length === 0,
    );
  });
  cdp.on("Network.responseReceivedExtraInfo", (event) => {
    const sent = entry(event.requestId);
    sent.status = event.statusCode;
    const setCookie = event.headers["Set-Cookie"] ?? event.headers["set-cookie"] ?? "";
    sent.setsStateCookie = setCookie.includes("fvoci_oidc_state");
  });
  cdp.on("Fetch.requestPaused", (event) => {
    void (async () => {
      if (event.networkId && event.responseStatusCode !== undefined) {
        const sent = entry(event.networkId);
        try {
          const body = await cdp.send("Fetch.getResponseBody", { requestId: event.requestId });
          const text = body.base64Encoded
            ? Buffer.from(body.body, "base64").toString("utf8")
            : body.body;
          sent.code = (JSON.parse(text) as { code?: string }).code ?? null;
        } catch {
          sent.code = null;
        }
      }
      await cdp
        .send("Fetch.continueRequest", { requestId: event.requestId })
        .catch(() => undefined);
    })();
  });
  await cdp.send("Network.enable");
  await cdp.send("Fetch.enable", {
    patterns: urls.map((url) => ({ urlPattern: url, requestStage: "Response" as const })),
  });
  return {
    last(url: string): SentPost | undefined {
      const id = [...order].reverse().find((candidate) => urlById.get(candidate) === url);
      return id ? byId.get(id) : undefined;
    },
    stop: async () => {
      await cdp.send("Fetch.disable").catch(() => undefined);
      await cdp.detach();
    },
  };
}

/** The server log without terminal colour codes. */
function serverLogLines(path: string): string[] {
  return readFileSync(path, "utf8")
    .replace(/\u001b\[[0-9;]*m/g, "")
    .split("\n");
}

function stateCookie(cookies: Cookie[]): Cookie | undefined {
  return cookies.find((c) => c.name === "fvoci_oidc_state");
}

// ---------------------------------------------------------------------------
// Shared first step of every group

async function prepareInstance(browser: Browser): Promise<void> {
  const c = config();
  // One issuer everywhere: the server's setting, the runner's config (which
  // the browser and this spec use) and the discovery document.
  expect(process.env.OIDC_GENERIC_ISSUER).toBe(c.issuer);
  expect(process.env.OIDC_ALLOW_INSECURE).toBe("1");
  const doc = await discovery();
  expect(doc.issuer).toBe(c.issuer);
  for (const endpoint of [doc.authorization_endpoint, doc.token_endpoint, doc.jwks_uri]) {
    expect(endpoint.startsWith(`${c.issuer}/`)).toBe(true);
  }
  observe("discovery", {
    issuer: doc.issuer,
    authorization_endpoint: doc.authorization_endpoint,
    token_endpoint: doc.token_endpoint,
    jwks_uri: doc.jwks_uri,
    token_endpoint_auth_methods_supported: doc.token_endpoint_auth_methods_supported,
    code_challenge_methods_supported: doc.code_challenge_methods_supported,
    authorization_response_iss_parameter_supported:
      doc.authorization_response_iss_parameter_supported,
  });
  observe("topology", {
    fvociOrigin,
    redirectUri,
    configuredIssuer: process.env.OIDC_GENERIC_ISSUER,
    keycloakOrigin: c.keycloakOrigin,
  });
  await registerRedirectUri();

  const { context, page } = await newPage(browser);
  await setupOwner(page);
  const providersResponse = await page.request.get("/api/v1/auth/providers");
  expect(providersResponse.status()).toBe(200);
  const providersText = await providersResponse.text();
  const secret = process.env.OIDC_GENERIC_CLIENT_SECRET ?? "";
  expect(
    secret.length > 0 && providersText.includes(secret),
    "the client secret stays on the server",
  ).toBe(false);
  const providers = JSON.parse(providersText) as { providers: unknown; workspaceSso: boolean };
  expect(providers.providers).toEqual([{ provider: "generic", label: c.label }]);
  expect(providers.workspaceSso).toBe(false);
  // A normal build trusts no entitlement issuer: local test-license SSO below
  // must never be mistaken for enabling workspace SSO in this release server.
  const sso = await page.request.get(`/api/v1/auth/sso?slug=${WORKSPACE.slug}`, {
    maxRedirects: 0,
  });
  expect(sso.status()).toBe(302);
  expect(new URL(sso.headers().location, fvociOrigin).pathname).toBe("/login");
  expect(new URL(sso.headers().location, fvociOrigin).searchParams.get("error")).toBe(
    "provider_not_configured",
  );
  observe("unentitledWorkspaceSso", {
    workspaceSso: false,
    status: sso.status(),
    error: "provider_not_configured",
  });
  observe("providers", providers);
  observe("servedWebAssets", await servedAssetsWithout(page, secret));
  await context.close();
}

/** Every file of the static root as this server serves it, plus the SPA entry routes. */
async function servedAssetsWithout(page: Page, secret: string): Promise<Record<string, number>> {
  const root = process.env.FVOCI_STATIC_DIR ?? "";
  expect(root.length > 0, "the harness serves a static root").toBe(true);
  const files = (
    readdirSync(root, { recursive: true, withFileTypes: true }) as Array<{
      name: string;
      parentPath: string;
      isFile(): boolean;
    }>
  )
    .filter((entry) => entry.isFile())
    .map((entry) =>
      path.relative(root, path.join(entry.parentPath, entry.name)).split(path.sep).join("/"),
    );
  const paths = ["/", "/login", "/settings/account", ...files.map((file) => `/${file}`)];
  let checked = 0;
  for (const asset of paths) {
    const response = await page.request.get(asset);
    expect(response.status(), asset).toBe(200);
    const body = await response.body();
    expect(secret.length > 0 && body.includes(secret), `${asset} holds the client secret`).toBe(
      false,
    );
    checked += 1;
  }
  expect(files).toContain("index.html");
  return { filesInStaticRoot: files.length, responsesChecked: checked, clientSecretFound: 0 };
}

// ---------------------------------------------------------------------------

test.describe.configure({ mode: "serial" });
test.skip(!kc, "opt-in: set by scripts/keycloak-oidc-e2e.sh (real Keycloak)");

test.afterAll(() => {
  if (!outDir || !kc) return;
  writeFileSync(
    `${outDir}/observations-${mode}.json`,
    `${JSON.stringify(observations, null, 2)}\n`,
  );
  // Server log excerpt: OIDC outcomes and the auth routes' request events
  // (the request trace logs route templates, never URIs or headers).
  const log = process.env.SERVER_LOG ?? "";
  if (log && existsSync(log)) {
    const lines = serverLogLines(log).filter((line) =>
      /oidc|\/api\/v1\/auth\/|listening on|ERROR/.test(line),
    );
    writeFileSync(`${outDir}/server-log-${mode}.txt`, `${lines.join("\n")}\n`);
  }
});

test.describe("sign-in, account linking, invitations, sign-out", () => {
  test.skip(mode !== "flows", "FVOCI_KC_E2E_MODE=flows");

  let ownerId = "";

  test("setup: provider listed, exact redirect URI registered, one issuer", async ({ browser }) => {
    await prepareInstance(browser);
    ownerId = sql(`SELECT id FROM fvoci.users WHERE email = '${OWNER.email}'`);
    expect(ownerId.length).toBe(36);
  });

  test("A: an unlinked Keycloak user is refused (oidc_not_linked); request shape", async ({
    browser,
  }) => {
    const { context, page, csp } = await newPage(browser);
    const usersBefore = userCount();
    await openLogin(page);
    const leg = keycloakLeg(page);
    await clickProvider(page);
    const authorization = new URL((await leg.authorization).url());
    const doc = await discovery();
    // The browser is sent to the discovery authorization endpoint.
    expect(`${authorization.origin}${authorization.pathname}`).toBe(doc.authorization_endpoint);
    const params = authorization.searchParams;
    const names = [...params.keys()].sort();
    expect(names).toEqual([
      "client_id",
      "code_challenge",
      "code_challenge_method",
      "nonce",
      "redirect_uri",
      "response_type",
      "scope",
      "state",
    ]);
    const random43 = (value: string | null) => /^[A-Za-z0-9_-]{43}$/.test(value ?? "");
    expect(params.get("response_type")).toBe("code");
    expect(params.get("client_id")).toBe(config().clientId);
    expect(params.get("redirect_uri")).toBe(redirectUri);
    expect(params.get("code_challenge_method")).toBe("S256");
    expect(random43(params.get("code_challenge")), "S256 challenge is 43 base64url chars").toBe(
      true,
    );
    expect(random43(params.get("state")), "state is 32 random bytes").toBe(true);
    expect(random43(params.get("nonce")), "nonce is 32 random bytes").toBe(true);
    expect(params.get("state") === params.get("nonce")).toBe(false);
    expect((params.get("scope") ?? "").split(" ").sort()).toEqual(["email", "openid", "profile"]);
    const secret = process.env.OIDC_GENERIC_CLIENT_SECRET ?? "";
    expect(secret.length > 0 && authorization.href.includes(secret)).toBe(false);
    observe("authorizationRequest", {
      endpoint: `${authorization.origin}${authorization.pathname}`,
      parameters: names,
      response_type: params.get("response_type"),
      scope: params.get("scope"),
      redirect_uri: params.get("redirect_uri"),
      code_challenge_method: params.get("code_challenge_method"),
      statePresent: random43(params.get("state")),
      noncePresent: random43(params.get("nonce")),
      clientSecretInUrl: false,
    });

    expect(await leg.pass(config().users.bob)).toBe("form");
    const callbackRequest = await leg.callback;
    const callbackParams = [...new URL(callbackRequest.url()).searchParams.keys()].sort();
    // RFC 9207: Keycloak sends `iss`, which the server compares to discovery.
    expect(new URL(callbackRequest.url()).searchParams.get("iss")).toBe(config().issuer);
    observe("callbackParameters", callbackParams);
    expect(await callbackResult(callbackRequest)).toEqual({
      status: 302,
      location: "/login?error=oidc_not_linked",
    });
    await expect(page.getByText(MSG.notLinked)).toBeVisible();
    expect(await me(page)).toBeNull();
    expect(stateCookie(await context.cookies(fvociOrigin))).toBeUndefined();
    expect(userCount()).toBe(usersBefore);
    expect(links()).toEqual([]);
    expect(csp).toEqual([]);
    observe("A_unlinked", { callback: "/login?error=oidc_not_linked", accountCreated: false });
    await context.close();
  });

  test("B: the password owner links Keycloak alice in account settings", async ({ browser }) => {
    const { context, page, csp } = await newPage(browser);
    await login(page, OWNER.email, OWNER.password);
    await openAccountSettings(page);
    const leg = keycloakLeg(page);
    await linkButton(page).click();
    expect(await leg.pass(config().users.alice)).toBe("form");
    expect(await callbackResult(await leg.callback)).toEqual({
      status: 302,
      location: "/settings/account?linked=1",
    });
    await expect(page.getByText(MSG.linked)).toBeVisible();
    await expect(page.getByText(`${config().label} — ${config().users.alice.email}`)).toBeVisible();
    const identities = await page.request.get("/api/v1/auth/identities");
    const items = (
      (await identities.json()) as { items: Array<{ provider: string; email: string }> }
    ).items;
    expect(items.map((i) => [i.provider, i.email])).toEqual([
      ["generic", config().users.alice.email],
    ]);
    const [link] = links();
    // The id_token `iss` the server verified is stored with the link.
    expect(link).toEqual({
      user: OWNER.email,
      provider: "generic",
      subject: await kcUserId("alice"),
      issuer: config().issuer,
      email: config().users.alice.email,
    });
    expect(userCount()).toBe(1);
    expect(csp).toEqual([]);
    observe("B_link", {
      callback: "/settings/account?linked=1",
      linkIssuerEqualsConfiguredIssuer: link.issuer === config().issuer,
      linkSubjectIsKeycloakUserId: true,
      users: userCount(),
    });
    await context.close();
  });

  test("B/D: sign out ends the app session; the provider signs in to the same account", async ({
    browser,
  }) => {
    const { context, page, csp } = await newPage(browser);
    // The owner's browser signs in with the password first, then signs out.
    await login(page, OWNER.email, OWNER.password);
    const before = await me(page);
    expect(before?.userId).toBe(ownerId);
    const saved = await context.storageState();
    const replayBefore = await playwrightRequest.newContext({ baseURL, storageState: saved });
    expect((await replayBefore.get("/api/v1/auth/me")).status()).toBe(200);
    await replayBefore.dispose();
    await page.goto("/");
    await logout(page);
    expect(await me(page)).toBeNull();
    // D: the cookie value captured before sign-out no longer works.
    const replay = await playwrightRequest.newContext({ baseURL, storageState: saved });
    const replayMe = await replay.get("/api/v1/auth/me");
    const replayWorkspaces = await replay.get("/api/v1/me/workspaces");
    expect(replayMe.status()).toBe(401);
    expect(replayWorkspaces.status()).toBe(401);
    await replay.dispose();

    // A: provider sign-in (Keycloak form: this browser has no Keycloak session yet).
    const first = await providerSignIn(page, config().users.alice);
    expect(first.via).toBe("form");
    expect(first.me.userId).toBe(ownerId);
    expect(first.me.email).toBe(OWNER.email);
    expect(await workspaceSlugs(page)).toEqual([WORKSPACE.slug]);
    expect(userCount()).toBe(1);

    // D: sign out again; this browser's Keycloak session stays (no single
    // logout): the next provider sign-in skips Keycloak's form.
    const signedIn = await context.storageState();
    await page.goto("/");
    await logout(page);
    const replayOidc = await playwrightRequest.newContext({ baseURL, storageState: signedIn });
    expect((await replayOidc.get("/api/v1/auth/me")).status()).toBe(401);
    await replayOidc.dispose();
    const again = await providerSignIn(page, null);
    expect(again.via).toBe("sso");
    expect(again.me.userId).toBe(ownerId);
    expect(userCount()).toBe(1);
    // Nor the other way round: ending alice's Keycloak sessions does not end
    // the FVOCI session (no back-channel logout).
    const kcLogout = await kcAdmin(`/users/${await kcUserId("alice")}/logout`, { method: "POST" });
    expect(kcLogout.status).toBe(204);
    expect(await kcSessionCount("alice")).toBe(0);
    expect((await me(page))?.userId).toBe(ownerId);
    expect(csp).toEqual([]);
    observe("BD_signout_signin", {
      passwordSessionReplayAfterLogout: replayMe.status(),
      passwordSessionReplayWorkspacesAfterLogout: replayWorkspaces.status(),
      oidcSessionReplayAfterLogout: 401,
      providerSignInSameAccount: true,
      firstProviderSignInKeycloakStep: first.via,
      reSignInKeycloakStep: again.via,
      keycloakSessionAfterFvociLogout: "kept (re-sign-in answered by Keycloak's session, no form)",
      fvociSessionAfterKeycloakAdminLogout: "still valid (no back-channel logout)",
      users: userCount(),
    });
    await context.close();
  });

  test("B: a second Keycloak user cannot be linked into the owner's account", async ({
    browser,
  }) => {
    const { context, page } = await newPage(browser);
    await login(page, OWNER.email, OWNER.password);
    await openAccountSettings(page);
    // The page offers unlink, not link, for a linked provider.
    await expect(page.getByRole("button", { name: "해제", exact: true })).toBeVisible();
    await expect(linkButton(page)).toHaveCount(0);
    // The same same-origin POST the link button makes, from this page.
    const leg = keycloakLeg(page);
    const started = await page.evaluate(async () => {
      const response = await fetch("/api/v1/auth/oidc/generic/link", {
        method: "POST",
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      });
      const body = (await response.json()) as { authorizationUrl?: string };
      const target = body.authorizationUrl;
      if (target) setTimeout(() => window.location.assign(target), 0);
      return response.status;
    });
    expect(started).toBe(200);
    expect(await leg.pass(config().users.bob)).toBe("form");
    expect(await callbackResult(await leg.callback)).toEqual({
      status: 302,
      location: "/settings/account?error=oidc_already_linked",
    });
    await expect(page.getByText(MSG.alreadyLinked)).toBeVisible();
    expect(linksOf(OWNER.email).map((l) => l.email)).toEqual([config().users.alice.email]);
    expect(userCount()).toBe(1);
    observe("B_second_identity", {
      callback: "/settings/account?error=oidc_already_linked",
      ownerLinks: 1,
    });
    await context.close();
  });

  test("C: an invitation accepted with the provider creates the member", async ({ browser }) => {
    const owner = await newPage(browser);
    await login(owner.page, OWNER.email, OWNER.password);
    const carol = config().users.carol;
    const token = await inviteViaUi(owner.page, carol.email);
    const usersBefore = userCount();

    const { context, page, csp } = await newPage(browser);
    await openInvitation(page, token);
    await expect(page.getByRole("heading", { name: /초대 수락/ })).toBeVisible();
    const leg = keycloakLeg(page);
    await page.getByRole("button", { name: config().label, exact: true }).click();
    expect(await leg.pass(carol)).toBe("form");
    expect(await callbackResult(await leg.callback)).toEqual({ status: 302, location: "/" });
    await page.waitForURL((u) => u.origin === fvociOrigin && u.pathname === "/");
    const member = await me(page);
    expect(member?.email).toBe(carol.email);
    expect(member?.hasPassword).toBe(false);
    expect(await workspaceSlugs(page)).toEqual([WORKSPACE.slug]);
    expect(membershipRole(carol.email)).toBe("member");
    expect(userCount()).toBe(usersBefore + 1);
    expect(linksOf(carol.email)).toEqual([
      {
        user: carol.email,
        provider: "generic",
        subject: await kcUserId("carol"),
        issuer: config().issuer,
        email: carol.email,
      },
    ]);
    await owner.page.goto(`/w/${WORKSPACE.slug}/settings`);
    await owner.page
      .locator("summary")
      .filter({ hasText: /^멤버$/ })
      .click();
    await expect(owner.page.getByText(carol.email)).toBeVisible();
    expect(csp).toEqual([]);
    observe("C_invite_match", {
      invitedEmail: carol.email,
      idpEmail: carol.email,
      idpEmailVerified: true,
      result: "member created",
      accountEmail: member?.email,
      hasPassword: member?.hasPassword,
      role: "member",
    });
    await context.close();
    await owner.context.close();
  });

  // ACC-1 (pending decision): the current behaviour is recorded, not changed.
  for (const probe of [
    { key: "C_invite_email_mismatch", invited: "kc-dave@example.com", user: "mallory" as const },
    { key: "C_invite_email_unverified", invited: "kc-erin@example.com", user: "erin" as const },
  ]) {
    test(`C (ACC-1 record): ${probe.key}`, async ({ browser }) => {
      const owner = await newPage(browser);
      await login(owner.page, OWNER.email, OWNER.password);
      const token = await inviteViaUi(owner.page, probe.invited);
      await owner.context.close();
      const idpUser = config().users[probe.user];
      const usersBefore = userCount();

      const { context, page } = await newPage(browser);
      await openInvitation(page, token);
      const leg = keycloakLeg(page);
      await page.getByRole("button", { name: config().label, exact: true }).click();
      expect(await leg.pass(idpUser)).toBe("form");
      const result = await callbackResult(await leg.callback);
      const current = result.location === "/" ? await me(page) : null;
      const accepted =
        sql(
          `SELECT accepted_at IS NOT NULL FROM fvoci.invitations WHERE email = '${probe.invited}'`,
        ) === "t";
      const observed = {
        invitedEmail: probe.invited,
        idpEmail: idpUser.email,
        idpEmailVerified: probe.user !== "erin",
        callback: result.location,
        invitationAccepted: accepted,
        accountEmail: current?.email ?? null,
        accountsCreated: userCount() - usersBefore,
        links: linksOf(probe.invited).map((l) => ({ email: l.email, issuer: l.issuer })),
        role: current ? membershipRole(probe.invited) : null,
      };
      observe(probe.key, observed);
      // Current behaviour: the invitation token decides; the account takes the
      // invited address and the link keeps the IdP's address, whether or not
      // the IdP marks it verified. Update with the ACC-1 decision.
      expect(observed).toEqual({
        invitedEmail: probe.invited,
        idpEmail: idpUser.email,
        idpEmailVerified: probe.user !== "erin",
        callback: "/",
        invitationAccepted: true,
        accountEmail: probe.invited,
        accountsCreated: 1,
        links: [{ email: idpUser.email, issuer: config().issuer }],
        role: "member",
      });
      await context.close();
    });
  }
});

test.describe("failure boundaries", () => {
  test.skip(mode !== "failures", "FVOCI_KC_E2E_MODE=failures");

  let ownerId = "";

  test("setup: owner links Keycloak alice; a second password member", async ({ browser }) => {
    await prepareInstance(browser);
    ownerId = sql(`SELECT id FROM fvoci.users WHERE email = '${OWNER.email}'`);
    const { context, page } = await newPage(browser);
    await login(page, OWNER.email, OWNER.password);
    await openAccountSettings(page);
    const leg = keycloakLeg(page);
    await linkButton(page).click();
    expect(await leg.pass(config().users.alice)).toBe("form");
    expect((await callbackResult(await leg.callback)).location).toBe("/settings/account?linked=1");
    const token = await inviteViaUi(page, PAT.email);
    await context.close();

    const pat = await newPage(browser);
    await openInvitation(pat.page, token);
    await pat.page.getByLabel("이메일").fill(PAT.email);
    await pat.page.getByLabel("성").fill(PAT.familyName);
    await pat.page.getByLabel("이름", { exact: true }).fill(PAT.givenName);
    await pat.page.getByLabel("비밀번호").fill(PAT.password);
    await pat.page.getByRole("button", { name: "수락" }).click();
    await expect(pat.page).toHaveURL(/\/$/);
    await pat.context.close();
    expect(userCount()).toBe(2);
  });

  test("tampered state creates no session; a restart signs in", async ({ browser }) => {
    const { context, page } = await newPage(browser);
    await openLogin(page);
    const hold = await holdCallback(page);
    const leg = keycloakLeg(page);
    await clickProvider(page);
    expect(await leg.pass(config().users.alice)).toBe("form");
    const held = new URL(await hold.url);
    await hold.release();
    const statesBefore = stateCount();
    const tampered = new URL(held);
    tampered.searchParams.set("state", randomBytes(32).toString("base64url"));
    const sessionsBefore = liveSessions(OWNER.email);
    const result = await visitCallback(page, tampered.toString());
    expect(result).toEqual({ status: 302, location: "/login?error=oidc_state_mismatch" });
    await expect(page.getByText(MSG.stateMismatch)).toBeVisible();
    expect(await me(page)).toBeNull();
    expect(liveSessions(OWNER.email)).toBe(sessionsBefore);
    // The real state was not consumed by the tampered request.
    expect(stateCount()).toBe(statesBefore);

    const restarted = await providerSignIn(page, null);
    expect(restarted.me.userId).toBe(ownerId);
    observe("F_tampered_state", {
      callback: result.location,
      sessionCreated: false,
      restart: { keycloakStep: restarted.via, signedInAsOwner: true },
    });
    await context.close();
  });

  test("a replayed callback creates no session (with and without the state cookie)", async ({
    browser,
  }) => {
    const { context, page } = await newPage(browser);
    await openLogin(page);
    const leg = keycloakLeg(page);
    await clickProvider(page);
    await leg.authorization;
    await page.locator("#username").waitFor({ state: "visible" });
    const cookie = stateCookie(await context.cookies(fvociOrigin));
    expect(cookie, "the start set the state cookie").toBeDefined();
    expect(cookie?.httpOnly).toBe(true);
    expect(cookie?.sameSite).toBe("Lax");
    expect(await leg.pass(config().users.alice)).toBe("form");
    const callback = await leg.callback;
    expect(await callbackResult(callback)).toEqual({ status: 302, location: "/" });
    expect((await me(page))?.userId).toBe(ownerId);
    await page.goto("/");
    await logout(page);

    const sessionsBefore = liveSessions(OWNER.email);
    const replay = await visitCallback(page, callback.url());
    expect(replay).toEqual({ status: 302, location: "/login?error=oidc_state_mismatch" });
    expect(await me(page)).toBeNull();
    await context.addCookies([cookie as Cookie]);
    const replayWithCookie = await visitCallback(page, callback.url());
    expect(replayWithCookie).toEqual({ status: 302, location: "/login?error=oidc_state_mismatch" });
    expect(await me(page)).toBeNull();
    expect(liveSessions(OWNER.email)).toBe(sessionsBefore);
    observe("F_replayed_callback", {
      withoutStateCookie: replay.location,
      withReplayedStateCookie: replayWithCookie.location,
      sessionCreated: false,
    });
    await context.close();
  });

  test("a callback delivered to another browser context does not sign in or link", async ({
    browser,
  }) => {
    // Sign-in: context X starts, context Y receives X's callback.
    const x = await newPage(browser);
    await openLogin(x.page);
    const hold = await holdCallback(x.page);
    const leg = keycloakLeg(x.page);
    await clickProvider(x.page);
    expect(await leg.pass(config().users.alice)).toBe("form");
    const heldUrl = await hold.url;
    await hold.release();

    const y = await newPage(browser);
    const other = await visitCallback(y.page, heldUrl);
    expect(other).toEqual({ status: 302, location: "/login?error=oidc_state_mismatch" });
    expect(await me(y.page)).toBeNull();
    await y.context.close();
    // The browser that started the flow still completes it.
    const own = await visitCallback(x.page, heldUrl);
    expect(own).toEqual({ status: 302, location: "/" });
    expect((await me(x.page))?.userId).toBe(ownerId);
    await x.context.close();

    // Link: the password member starts a link to Keycloak bob; the owner's
    // signed-in browser receives that callback.
    const pat = await newPage(browser);
    await login(pat.page, PAT.email, PAT.password);
    await openAccountSettings(pat.page);
    const patHold = await holdCallback(pat.page);
    const patLeg = keycloakLeg(pat.page);
    await linkButton(pat.page).click();
    expect(await patLeg.pass(config().users.bob)).toBe("form");
    const patHeld = await patHold.url;
    await patHold.release();
    await pat.context.close();

    const owner = await newPage(browser);
    await login(owner.page, OWNER.email, OWNER.password);
    const delivered = await visitCallback(owner.page, patHeld);
    expect(delivered).toEqual({ status: 302, location: "/login?error=oidc_state_mismatch" });
    expect(linksOf(OWNER.email).map((l) => l.email)).toEqual([config().users.alice.email]);
    expect(linksOf(PAT.email)).toEqual([]);
    await owner.context.close();
    observe("F_other_browser_context", {
      signInCallbackInOtherContext: other.location,
      originalContextCompletes: own.location,
      linkCallbackInOtherSignedInContext: delivered.location,
      linksChanged: false,
    });
  });

  test("an identity linked to another account cannot be linked again", async ({ browser }) => {
    const { context, page } = await newPage(browser);
    await login(page, PAT.email, PAT.password);
    await openAccountSettings(page);
    const leg = keycloakLeg(page);
    await linkButton(page).click();
    expect(await leg.pass(config().users.alice)).toBe("form");
    const result = await callbackResult(await leg.callback);
    expect(result).toEqual({
      status: 302,
      location: "/settings/account?error=oidc_already_linked",
    });
    await expect(page.getByText(MSG.alreadyLinked)).toBeVisible();
    expect(linksOf(PAT.email)).toEqual([]);
    expect(linksOf(OWNER.email).map((l) => l.email)).toEqual([config().users.alice.email]);
    observe("F_identity_taken", { callback: result.location });
    await context.close();
  });

  test("access denied at Keycloak (declined terms) fails cleanly; a restart links", async ({
    browser,
  }) => {
    const { context, page, csp } = await newPage(browser);
    await login(page, PAT.email, PAT.password);
    await openAccountSettings(page);
    const tina = config().users.tina;
    const leg = keycloakLeg(page);
    await linkButton(page).click();
    expect(await leg.pass(tina)).toBe("form");
    await page.locator("#kc-decline").click();
    const denied = await leg.callback;
    const deniedParams = new URL(denied.url()).searchParams;
    expect(deniedParams.get("error")).toBe("access_denied");
    expect(deniedParams.has("code")).toBe(false);
    const result = await callbackResult(denied);
    expect(result).toEqual({
      status: 302,
      location: "/settings/account?error=oidc_provider_error",
    });
    await expect(page.getByText(MSG.providerError)).toBeVisible();
    expect(linksOf(PAT.email)).toEqual([]);

    const retry = keycloakLeg(page);
    await linkButton(page).click();
    const step = await retry.pass(tina);
    await page.locator("#kc-accept").click();
    const linked = await callbackResult(await retry.callback);
    expect(linked).toEqual({ status: 302, location: "/settings/account?linked=1" });
    await expect(page.getByText(MSG.linked)).toBeVisible();
    expect(linksOf(PAT.email).map((l) => [l.email, l.issuer])).toEqual([
      [tina.email, config().issuer],
    ]);
    expect(csp).toEqual([]);
    observe("F_access_denied", {
      keycloakRedirect: {
        error: deniedParams.get("error"),
        error_description: deniedParams.get("error_description"),
        parameters: [...deniedParams.keys()].sort(),
      },
      callback: result.location,
      linked: false,
      restart: { keycloakStep: step, callback: linked.location, linked: true },
    });
    await context.close();
  });

  test("cross-origin link and invite starts are refused", async ({ browser }) => {
    const { context, page } = await newPage(browser);
    await login(page, OWNER.email, OWNER.password);
    const token = await inviteViaUi(page, "kc-xorigin@example.com");
    const statesBefore = stateCount();
    const linksBefore = links();

    const linkUrl = `${fvociOrigin}/api/v1/auth/oidc/generic/link`;
    const startUrl = `${fvociOrigin}/api/v1/auth/oidc/generic/start`;
    const html = `<!doctype html><meta charset="utf-8"><title>other origin</title>
<form id="link" method="post" action="${linkUrl}"><button>link</button></form>
<form id="invite" method="post" action="${startUrl}"><input name="invitation" value="${token}"><button>invite</button></form>
<script>
window.post = async (url, body) => {
  try {
    const response = await fetch(url, { method: "POST", credentials: "include", body });
    return "readable " + response.status;
  } catch (error) {
    return "unreadable (" + error.name + ")";
  }
};
</script>`;
    const server: Server = createServer((_request, response) => {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      response.end(html);
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const otherOrigin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
    const network = await watchPosts(page, [linkUrl, startUrl]);
    try {
      await page.goto(`${otherOrigin}/`);
      const refused: Record<string, unknown> = {};
      // Script requests: the page cannot read the answer (no CORS headers);
      // the browser's network log shows what was sent and what came back.
      for (const [name, url, body] of [
        ["fetchLink", linkUrl, null],
        ["fetchInvite", startUrl, `invitation=${token}`],
      ] as const) {
        const pageSees = await page.evaluate(
          ([target, form]) =>
            (
              window as unknown as { post: (u: string, b?: URLSearchParams) => Promise<string> }
            ).post(target, form === null ? undefined : new URLSearchParams(form)),
          [url, body] as const,
        );
        expect(pageSees).toBe("unreadable (TypeError)");
        await expect.poll(() => network.last(url)?.status).toBe(403);
        await expect.poll(() => network.last(url)?.code).toBe("origin_mismatch");
        const sent = network.last(url);
        expect(sent?.origin).toBe(otherOrigin);
        expect(sent?.sessionCookieSent).toBe(true);
        expect(sent?.setsStateCookie).toBe(false);
        refused[name] = {
          status: sent?.status,
          code: sent?.code,
          sentOrigin: sent?.origin,
          sessionCookieSent: true,
          pageSees,
        };
      }
      // Form submissions: a top-level navigation to FVOCI with the page's origin.
      for (const [name, form, url] of [
        ["formLink", "#link", linkUrl],
        ["formInvite", "#invite", startUrl],
      ] as const) {
        await page.goto(`${otherOrigin}/`);
        const response = page.waitForResponse((r) => r.url() === url);
        await page.locator(`${form} button`).click();
        const answered = await response;
        expect(answered.status()).toBe(403);
        expect(await problemCode(answered)).toBe("origin_mismatch");
        await expect.poll(() => network.last(url)?.status).toBe(403);
        await expect.poll(() => network.last(url)?.code).toBe("origin_mismatch");
        const sent = network.last(url);
        expect(sent?.origin).toBe(otherOrigin);
        expect(sent?.sessionCookieSent).toBe(true);
        expect(sent?.setsStateCookie).toBe(false);
        await page.waitForLoadState();
        expect(new URL(page.url()).origin).toBe(fvociOrigin);
        refused[name] = {
          status: answered.status(),
          code: "origin_mismatch",
          sentOrigin: sent?.origin,
          sessionCookieSent: true,
          navigatedToKeycloak: false,
        };
      }
      expect(stateCookie(await context.cookies(fvociOrigin))).toBeUndefined();
      expect(stateCount()).toBe(statesBefore);
      expect(links()).toEqual(linksBefore);
      expect(
        sql(
          `SELECT accepted_at IS NULL FROM fvoci.invitations WHERE email = 'kc-xorigin@example.com'`,
        ),
      ).toBe("t");
      // The owner's session was attached (same site, other origin) and is intact.
      expect((await me(page))?.userId).toBe(ownerId);
      observe("F_cross_origin", { otherOrigin, sameSite: true, ...refused, statesIssued: 0 });
    } finally {
      await network.stop();
      await new Promise<void>((resolve) => server.close(() => resolve()));
      await context.close();
    }
  });
});

test.describe("wrong client secret on the server", () => {
  test.skip(mode !== "wrong-secret", "FVOCI_KC_E2E_MODE=wrong-secret");

  test("the token exchange fails clearly: error shown, no session, reason logged", async ({
    browser,
  }) => {
    await prepareInstance(browser);
    const { context, page } = await newPage(browser);
    await openLogin(page);
    const leg = keycloakLeg(page);
    await clickProvider(page);
    expect(await leg.pass(config().users.alice)).toBe("form");
    const result = await callbackResult(await leg.callback);
    expect(result).toEqual({ status: 302, location: "/login?error=oidc_provider_error" });
    await expect(page.getByText(MSG.providerError)).toBeVisible();
    expect(await me(page)).toBeNull();
    expect(userCount()).toBe(1);
    expect(links()).toEqual([]);

    const log = readFileSync(process.env.SERVER_LOG ?? "", "utf8");
    const failures = serverLogLines(process.env.SERVER_LOG ?? "")
      .filter((line) => line.includes("oidc.exchange_failed"))
      .map((line) => line.replace(/^\S+\s+/, ""));
    expect(failures.length).toBe(1);
    expect(failures[0]).toContain("provider response: token error");
    const secret = process.env.OIDC_GENERIC_CLIENT_SECRET ?? "";
    expect(secret.length > 0 && log.includes(secret), "the server log never holds the secret").toBe(
      false,
    );
    const events = await kcAdmin(`/events?type=CODE_TO_TOKEN_ERROR&client=${config().clientId}`);
    const errors = ((await events.json()) as Array<{ error?: string }>).map((e) => e.error);
    expect(errors).toContain("invalid_client_credentials");
    observe("F_wrong_client_secret", {
      callback: result.location,
      sessionCreated: false,
      serverLog: failures,
      keycloakEventErrors: errors,
    });
    await context.close();
  });
});
