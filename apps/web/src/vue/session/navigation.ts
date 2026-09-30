import { isVueAppPath } from "@/app-boundary";
import type { Router } from "vue-router";

// Leaving the Vue app is a full page load: every other page is the React
// app's, and the two apps share no runtime (src/app-boundary.ts).

/** Replaces the current page with `path` (a redirect: no history entry). */
export function redirectTo(path: string): void {
  window.location.replace(path);
}

/** A live Vue-app path stays in this app; any other href is a full page load. */
export function followAppHref(href: string, router: Router): void {
  if (isVueAppPath(href)) void router.push(href);
  else window.location.assign(href);
}

/** Browser pieces `leaveTo` uses; tests pass their own. */
export interface LeaveEnvironment {
  assign(url: string): void;
  push(path: string): void;
}

const BROWSER_LEAVE: LeaveEnvironment = {
  assign: (url) => window.location.assign(url),
  push: (path) => window.location.assign(path),
};

/**
 * Goes to `path`. A live Vue page is `push` (in-app); anything else is a
 * full load (`assign`), because the React app owns it. After deleting a
 * project the projects list is still React, so this is `location.assign`.
 */
export function leaveTo(path: string, env: LeaveEnvironment = BROWSER_LEAVE): void {
  const pathname = path.split(/[?#]/, 1)[0] ?? "";
  if (isVueAppPath(pathname)) env.push(path);
  else env.assign(path);
}

/** Login with the current page as `returnTo` (the login page checks it with safeReturnTo). */
export function loginPath(location: Pick<Location, "pathname" | "search" | "hash">): string {
  return `/login?returnTo=${encodeURIComponent(`${location.pathname}${location.search}${location.hash}`)}`;
}
