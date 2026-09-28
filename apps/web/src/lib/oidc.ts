// Adapted from fvoci/FVOCI apps/web/src/lib/oidc-error.ts plus the OIDC
// browser-navigation helpers of features/auth/{login,invite}.tsx and
// routes/login.tsx (`#mfa=` fragment).
import { t } from "@fvoci/i18n";
import type { components } from "@/generated/api";
import { consentGate, ProblemError, problemMessage } from "@/lib/api";

type OidcAuthorizationOutput = components["schemas"]["OidcAuthorizationOutput"];

export const OIDC_ERROR_CODES = [
  "oidc_not_linked",
  "oidc_state_mismatch",
  "oidc_provider_error",
  "oidc_already_linked",
  "oidc_invitation_invalid",
  "oidc_last_method",
] as const;

export type OidcErrorCode = (typeof OIDC_ERROR_CODES)[number];

export function isOidcErrorCode(code: string): code is OidcErrorCode {
  return (OIDC_ERROR_CODES as readonly string[]).includes(code);
}

/** Callback redirect `?error=` code → message. The wire code is the catalog key. */
export function oidcErrorMessage(code: string | null | undefined): string | null {
  if (!code) return null;
  return isOidcErrorCode(code) ? t(code) : t("oidc_fallback");
}

/**
 * The OIDC callback hands a pending MFA token over as `#mfa=<token>` so it never
 * reaches server logs or Referer. Returns the token, or null when absent.
 */
export function readMfaFragment(hash: string): string | null {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  if (raw === "") return null;
  const token = new URLSearchParams(raw).get("mfa");
  return token && token.length > 0 ? token : null;
}

/**
 * Reads the `#mfa=` token once and clears the fragment from the address bar
 * (keeping the router's history state) so a reload or shared URL cannot reuse it.
 */
export function takeMfaFragment(): string | null {
  if (typeof window === "undefined") return null;
  const token = readMfaFragment(window.location.hash);
  if (token !== null) {
    window.history.replaceState(
      window.history.state,
      "",
      `${window.location.pathname}${window.location.search}`,
    );
  }
  return token;
}

export type ConsentItem = { kind: string; version: string | number };

/** Plain browser navigation target for `GET /auth/oidc/{provider}/start` (sign-in only). */
export function oidcStartHref(provider: string): string {
  return `/api/v1/auth/oidc/${encodeURIComponent(provider)}/start`;
}

/**
 * Accepting an invitation with a provider is a same-origin POST to the start
 * route (the server refuses it from a GET, another origin or no origin); the
 * token and consents travel as urlencoded fields, never in the URL.
 */
export function oidcInviteStartForm(
  provider: string,
  invitation: { token: string; consents: readonly ConsentItem[] },
): { action: string; fields: { invitation: string; consents: string } } {
  return {
    action: oidcStartHref(provider),
    fields: {
      invitation: invitation.token,
      consents: JSON.stringify(invitation.consents),
    },
  };
}

/** POST target for linking a provider to the signed-in account. */
export function oidcLinkAction(provider: string): string {
  return `/api/v1/auth/oidc/${encodeURIComponent(provider)}/link`;
}

export type OidcStartDeps = {
  fetch: (input: string, init: RequestInit) => Promise<Response>;
  navigate: (url: string) => void;
};

function browserStartDeps(): OidcStartDeps {
  return {
    fetch: async (input, init) => consentGate(await globalThis.fetch(input, init)),
    navigate: (url) => window.location.assign(url),
  };
}

function problemCode(body: unknown): string | undefined {
  if (body === null || typeof body !== "object") return undefined;
  const code = (body as { code?: unknown }).code;
  return typeof code === "string" ? code : undefined;
}

function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:";
  } catch {
    return false;
  }
}

/**
 * The POST starts (invite, link): a same-origin `fetch` answers
 * `{authorizationUrl}` and sets the state cookie, then the page navigates to
 * the provider by script. Not a form submission: under the app's
 * `Referrer-Policy: no-referrer` a form navigation sends `Origin: null`, which
 * the server refuses, and its redirect to the provider's origin would be
 * blocked by the CSP `form-action 'self'` (Chromium, WebKit). A `fetch`
 * (mode cors) sends the page's origin. Throws a `ProblemError` and stays put
 * on any failure.
 */
export async function startOidcPost(
  action: string,
  body: URLSearchParams | undefined,
  deps: OidcStartDeps = browserStartDeps(),
): Promise<void> {
  const response = await deps.fetch(action, {
    method: "POST",
    credentials: "same-origin",
    headers: { Accept: "application/json" },
    ...(body ? { body } : {}),
  });
  const data: unknown = await response.json().catch(() => null);
  if (!response.ok) throw new ProblemError(response.status, problemCode(data));
  const url = (data as Partial<OidcAuthorizationOutput> | null)?.authorizationUrl;
  if (typeof url !== "string" || !isHttpUrl(url)) throw new ProblemError(500);
  deps.navigate(url);
}

/** Invite page: accept the invitation with `provider`. */
export function startOidcInvite(
  provider: string,
  invitation: { token: string; consents: readonly ConsentItem[] },
  deps?: OidcStartDeps,
): Promise<void> {
  const start = oidcInviteStartForm(provider, invitation);
  return startOidcPost(start.action, new URLSearchParams(start.fields), deps);
}

/** Account settings: link `provider` to the signed-in account. */
export function startOidcLink(provider: string, deps?: OidcStartDeps): Promise<void> {
  return startOidcPost(oidcLinkAction(provider), undefined, deps);
}

export type OidcStartUi = {
  setPending: (provider: string | null) => void;
  setError: (message: string | null) => void;
};

/**
 * Click on a provider button that starts a POST flow: clears the last error
 * and marks the provider pending while the page leaves for the provider; on
 * failure it shows the problem and releases the buttons.
 */
export async function clickOidcStart(
  provider: string,
  start: () => Promise<void>,
  ui: OidcStartUi,
  fallback: Parameters<typeof t>[0],
): Promise<void> {
  ui.setError(null);
  ui.setPending(provider);
  try {
    await start();
  } catch (err) {
    ui.setError(problemMessage(err, fallback));
    ui.setPending(null);
  }
}

export const WORKSPACE_SSO_ACTION = "/api/v1/auth/sso";

// The server's workspace slug rule (`normalize_slug`: trim, NFKC, then this).
const WORKSPACE_SLUG = /^[a-z0-9-]{2,32}$/;

export function workspaceSsoHref(slug: string): string {
  return `${WORKSPACE_SSO_ACTION}?slug=${encodeURIComponent(slug)}`;
}

/** Catalog key of a slug the server would refuse. */
export type WorkspaceSsoSlugIssue = "form.too_small" | "form.invalid";

/**
 * Login page "SSO로 로그인": the server resolves the workspace slug and
 * answers 302 to that workspace's IdP. The page navigates there by script, a
 * top-level navigation: a form submission's redirect to the IdP's origin is
 * blocked by the CSP `form-action 'self'` (Chromium). Returns the field
 * problem and stays put, or navigates and returns null.
 */
export function startWorkspaceSso(
  input: string,
  navigate: (url: string) => void = (url) => window.location.assign(url),
): WorkspaceSsoSlugIssue | null {
  const slug = input.trim().normalize("NFKC");
  if (slug === "") return "form.too_small";
  if (!WORKSPACE_SLUG.test(slug)) return "form.invalid";
  navigate(workspaceSsoHref(slug));
  return null;
}
