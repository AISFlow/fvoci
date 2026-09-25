// Adapted from fvoci/FVOCI apps/web/src/lib/oidc-error.ts plus the OIDC
// browser-navigation helpers of features/auth/{login,invite}.tsx and
// routes/login.tsx (`#mfa=` fragment).
import { t } from "@fvoci/i18n";

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

/** Plain browser navigation target for `GET /auth/oidc/{provider}/start`. */
export function oidcStartHref(
  provider: string,
  invitation?: { token: string; consents: readonly ConsentItem[] },
): string {
  const base = `/api/v1/auth/oidc/${encodeURIComponent(provider)}/start`;
  if (!invitation) return base;
  const query = new URLSearchParams({
    invitation: invitation.token,
    consents: JSON.stringify(invitation.consents),
  });
  return `${base}?${query.toString()}`;
}

/** Form POST target for linking a provider to the signed-in account. */
export function oidcLinkAction(provider: string): string {
  return `/api/v1/auth/oidc/${encodeURIComponent(provider)}/link`;
}

export const WORKSPACE_SSO_ACTION = "/api/v1/auth/sso";
