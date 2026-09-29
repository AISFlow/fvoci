/**
 * The Vue app's route paths. src/app-boundary.ts must send exactly these
 * paths to the Vue app (src/app-boundary.test.ts checks both agree).
 */
export const VUE_ROUTE_PATHS = {
  projectGantt: "/w/:slug/:ref/gantt",
  // The wiki document refs of lib/href.ts parseWikiRef; route paths match
  // case-insensitively, as the boundary does.
  wikiDocument: "/w/:slug/:ref(wiki-[1-9]\\d{0,8})",
  login: "/login",
  // Invite: add `invite: "/invite/:token"` here in the same change as
  // apps/web/src/app-boundary.ts `/^\/invite\/[^/]+\/?$/i` (app-boundary.test.ts).
} as const;
