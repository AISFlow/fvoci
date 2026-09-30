import { isLocalAppPath as isVueAppPath } from "@/vue/route-paths";
import type { Router } from "vue-router";

// Local SPA hrefs, including fallback paths, use the Vue router. External
// targets still use a browser navigation.

/** Replaces the current page with `path` (a redirect: no history entry). */
export function redirectTo(path: string): void {
  window.location.replace(path);
}

/** Local app hrefs stay in Vue; external hrefs use a full page load. */
export function followAppHref(href: string, router: Router): void {
  if (isVueAppPath(href.split(/[?#]/, 1)[0] ?? "")) router.push(href).catch(reportError);
  else window.location.assign(href);
}

/** Browser pieces `leaveTo` uses; tests pass their own. */
export interface LeaveEnvironment {
  assign(url: string): void;
  push(path: string): void;
}

const BROWSER_LEAVE: LeaveEnvironment = {
  assign: (url) => {
    window.location.assign(url);
  },
  push: (path) => {
    window.location.assign(path);
  },
};

/**
 * Goes to `path`: local SPA paths use `push`, including the router fallback;
 * other targets use a full load (`assign`).
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
