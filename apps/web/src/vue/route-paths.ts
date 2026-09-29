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
} as const;

/**
 * Vue pages that exist as lazy chunks but are not live: src/app-boundary.ts
 * still sends these paths to the React app, so a Vue in-app navigation to
 * them is a full page load (router.ts afterEach).
 */
export const STAGED_VUE_ROUTE_PATHS = {
  home: "/",
  legal: "/legal/:kind",
  serviceInfo: "/service-info",
} as const;
