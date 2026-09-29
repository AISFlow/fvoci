// Leaving the Vue app is a full page load: every other page is the React
// app's, and the two apps share no runtime (src/app-boundary.ts).

/** Replaces the current page with `path` (a redirect: no history entry). */
export function redirectTo(path: string): void {
  window.location.replace(path);
}

/** Login with the current page as `returnTo` (the login page checks it with safeReturnTo). */
export function loginPath(location: Pick<Location, "pathname" | "search" | "hash">): string {
  return `/login?returnTo=${encodeURIComponent(`${location.pathname}${location.search}${location.hash}`)}`;
}
