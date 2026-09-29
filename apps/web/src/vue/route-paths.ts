/**
 * The Vue app's route paths. src/app-boundary.ts must send exactly these
 * paths to the Vue app (src/app-boundary.test.ts checks both agree).
 */
export const VUE_ROUTE_PATHS = {
  projectGantt: "/w/:slug/:ref/gantt",
  // The wiki document refs of lib/href.ts parseWikiRef; route paths match
  // case-insensitively, as the boundary does.
  wikiDocument: "/w/:slug/:ref(wiki-[1-9]\\d{0,8})",
} as const;

/**
 * Project home overview (`/w/:slug/GNT`). Declared on the Vue router (same
 * `/w/:slug/:ref` shape as the staged collection routes) but not live:
 * src/app-boundary.ts still boots React. Coordinator later adds this regex
 * to VUE_APP_PATHS and this path to VUE_ROUTE_PATHS in the same change:
 *
 *   /^\/w\/[^/]+\/(?!(?:projects|search|wiki|trash|my-tasks|notifications|settings|a)\/?$)(?![^/]*-\d+$)[A-Za-z][A-Za-z0-9-]{1,31}\/?$/i
 *
 * Two segments only, so it does not take wiki refs (`wiki-3`) or
 * `/gantt` `/tasks` `/board` `/calendar` `/table` `/settings`. Reserved
 * workspace segments (`/projects`, `/search`, `/wiki`, …) stay React.
 */
export const STAGED_VUE_ROUTE_PATHS = {
  projectHome: "/w/:slug/:ref",
} as const;

/** The boundary regex quoted above; tests pin what it would send to Vue. */
export const STAGED_PROJECT_HOME_PATH =
  /^\/w\/[^/]+\/(?!(?:projects|search|wiki|trash|my-tasks|notifications|settings|a)\/?$)(?![^/]*-\d+$)[A-Za-z][A-Za-z0-9-]{1,31}\/?$/i;
