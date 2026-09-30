import { isLocalAppPath as isVueAppPath } from "@/vue/route-paths";
import type { Router } from "vue-router";

/** Replaces the browser document with `path` without adding a history entry. */
export function redirectTo(path: string): void {
  window.location.replace(path);
}

/** Follows local hrefs (including fallback paths) in Vue; external hrefs use a full load. */
export function followAppHref(href: string, router: Router): void {
  if (isVueAppPath(href.split(/[?#]/, 1)[0] ?? "")) router.push(href).catch(reportError);
  else window.location.assign(href);
}

/** Navigation operations supplied by the caller of `leaveTo`. */
export interface LeaveEnvironment {
  assign(url: string): void;
  push(path: string): void;
}

/**
 * Dispatches local paths (including fallback paths) to the caller's `push`;
 * other targets use the caller's `assign`.
 */
export function leaveTo(path: string, env: LeaveEnvironment): void {
  const pathname = path.split(/[?#]/, 1)[0] ?? "";
  if (isVueAppPath(pathname)) env.push(path);
  else env.assign(path);
}

/** Login with the current page as `returnTo` (the login page checks it with safeReturnTo). */
export function loginPath(location: Pick<Location, "pathname" | "search" | "hash">): string {
  return `/login?returnTo=${encodeURIComponent(`${location.pathname}${location.search}${location.hash}`)}`;
}
