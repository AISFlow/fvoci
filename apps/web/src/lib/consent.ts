// Consent gate helpers (source apps/web/src/lib/api.ts 428 branch and
// apps/web/src/routes/consent.tsx safeReturnTo). Kept import-free so the
// node unit tests can load it directly.

export const CONSENT_PATH = "/consent";

/** Pages that only lead into the app; after consenting the user goes home instead. */
const ENTRY_PATHS = new Set([CONSENT_PATH, "/login", "/setup", "/reset-password"]);

/** Where the consent prompt sends the user back to: the page that hit 428. */
export function consentUrl(location: { pathname: string; search: string; hash: string }): string {
  // Login refetches before it navigates home, so the gate can fire on /login.
  const here = ENTRY_PATHS.has(location.pathname)
    ? "/"
    : `${location.pathname}${location.search}${location.hash}`;
  return `${CONSENT_PATH}?returnTo=${encodeURIComponent(here)}`;
}

/**
 * Open-redirect guard: `startsWith("/")` alone lets `//evil.example` through
 * (a protocol-relative URL). Resolve against the current origin and keep only
 * same-origin path/search/hash; anything else falls back to "/".
 */
export function safeReturnTo(value: string | null, origin: string): string {
  if (!value) return "/";
  try {
    const url = new URL(value, origin);
    if (url.origin !== origin) return "/";
    // Returning to the prompt (a loop) or to an auth entry page is never useful.
    if (ENTRY_PATHS.has(url.pathname)) return "/";
    return url.pathname + url.search + url.hash;
  } catch {
    return "/";
  }
}

/** True when a problem body is the legal consent gate. */
export function isConsentRequired(status: number, body: unknown): boolean {
  return (
    status === 428 &&
    typeof body === "object" &&
    body !== null &&
    (body as { code?: unknown }).code === "consent_required"
  );
}
