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
  // Remaining auth pages: add these in the same change as app-boundary.ts
  // (app-boundary.test.ts requires both to agree). Boot still sends them to React.
  //   resetPassword: "/reset-password",  /^\/reset-password\/?$/i
  //   magicLink: "/magic-link",          /^\/magic-link\/?$/i
  //   confirmEmail: "/confirm-email",    /^\/confirm-email\/?$/i
  //   cancelWithdraw: "/cancel-withdraw",/^\/cancel-withdraw\/?$/i
  //   consent: "/consent",               /^\/consent\/?$/i
  home: "/",
  legal: "/legal/:kind",
  serviceInfo: "/service-info",
  invite: "/invite/:token",
  setup: "/setup",
} as const;
