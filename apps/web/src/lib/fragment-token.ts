/**
 * One-time tokens carried in URL fragments (`#token`) or `/invite/<token>`
 * paths: moved immediately out of the address bar and browser history into
 * tab-scoped `sessionStorage` so they survive a reload in the same tab without
 * lingering in `location.href` or history entries, and cleared once consumed
 * or rejected.
 */

const PREFIX = 'fvoci:token:';
/** How long a kept token survives a reload in the same tab (30 minutes). */
const KEPT_FOR = 30 * 60 * 1000;
/** Fallback when `sessionStorage` is blocked by the browser in this tab. */
const memoryFallback = new Map<string, { token: string; at: number }>();

function readKept(scope: string, now: number): string {
  try {
    const raw = sessionStorage.getItem(`${PREFIX}${scope}`);
    if (!raw) return '';
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== 'object' || parsed === null) return '';
    const { token, at } = parsed as Record<string, unknown>;
    if (typeof token !== 'string' || token === '' || typeof at !== 'number') return '';
    if (now - at < 0 || now - at >= KEPT_FOR) {
      sessionStorage.removeItem(`${PREFIX}${scope}`);
      return '';
    }
    return token;
  } catch {
    const entry = memoryFallback.get(scope);
    if (!entry) return '';
    if (now - entry.at < 0 || now - entry.at >= KEPT_FOR) {
      memoryFallback.delete(scope);
      return '';
    }
    return entry.token;
  }
}

function writeKept(scope: string, token: string, now: number): void {
  try {
    sessionStorage.setItem(`${PREFIX}${scope}`, JSON.stringify({ token, at: now }));
    memoryFallback.delete(scope);
  } catch {
    memoryFallback.set(scope, { token, at: now });
  }
}

function stripHash(): void {
  if (location.hash !== '') {
    history.replaceState(history.state, '', `${location.pathname}${location.search}`);
  }
}

/**
 * Reads a one-time token from `location.hash`, strips `#...` from the address
 * bar and history immediately, and keeps a short-lived copy in `sessionStorage`
 * for reloads in the same tab.
 */
export function takeFragmentToken(scope: string, now: number = Date.now()): string {
  const fromHash = location.hash.slice(1);
  if (fromHash !== '') {
    writeKept(scope, fromHash, now);
    stripHash();
    return fromHash;
  }
  return readKept(scope, now);
}

/** Reads the kept fragment token for `scope` without mutating history or storage. */
export function readFragmentToken(scope: string, now: number = Date.now()): string {
  return readKept(scope, now);
}

/** Clears the kept fragment token for `scope` and ensures no hash remains in the URL. */
export function forgetFragmentToken(scope: string): void {
  memoryFallback.delete(scope);
  try {
    sessionStorage.removeItem(`${PREFIX}${scope}`);
  } catch {
    // Nothing to clear.
  }
  stripHash();
}

const INVITE_SCOPE = 'invite';

/**
 * Keeps an invitation token from `/invite/<token>`, replacing the browser
 * entry with `cleanPath` (`/invite`), or reads back the kept token on reload.
 */
export function takeInviteToken(
  fromRoute: string | undefined,
  cleanPath: string,
  now: number = Date.now(),
): string {
  if (fromRoute !== undefined && fromRoute !== '') {
    writeKept(INVITE_SCOPE, fromRoute, now);
    if (location.pathname !== cleanPath) {
      history.replaceState(history.state, '', `${cleanPath}${location.search}${location.hash}`);
    }
    return fromRoute;
  }
  return readKept(INVITE_SCOPE, now);
}

/** Reads the kept invitation token without mutating history or storage. */
export function readInviteToken(now: number = Date.now()): string {
  return readKept(INVITE_SCOPE, now);
}

/** Clears any kept invitation token once joined or refused as unavailable. */
export function forgetInviteToken(): void {
  memoryFallback.delete(INVITE_SCOPE);
  try {
    sessionStorage.removeItem(`${PREFIX}${INVITE_SCOPE}`);
  } catch {
    // Nothing to clear.
  }
}
