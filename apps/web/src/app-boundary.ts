// The temporary React/Vue boundary. One index.html serves every SPA path (the
// server's fallback and head injection only know that file); src/boot.ts loads
// the Vue app for the paths below and the React app for everything else.
// Crossing the boundary is always a full page load: the two apps share no
// runtime objects, only the session cookie and the framework-neutral modules.
// A path moves here when its Vue flow is accepted and its React page is removed.

/**
 * Path patterns the Vue app renders (src/vue/router.ts declares the same
 * routes). Case-insensitive, as vue-router and React Router match by default.
 */
export const VUE_APP_PATHS: readonly RegExp[] = [
  // Project Gantt: /w/:slug/:ref/gantt
  /^\/w\/[^/]+\/[^/]+\/gantt\/?$/i,
  // Wiki document: /w/:slug/WIKI-<n>, the refs parseWikiRef (lib/href.ts)
  // accepts (1-9 digits, no leading zero, prefix in any case). Other spellings
  // that the React router decodes to one of these (percent-encoded) reach
  // WorkspaceRefPage, which reloads the canonical path.
  /^\/w\/[^/]+\/wiki-[1-9]\d{0,8}\/?$/i,
  // Login: logout landing, MFA step, OIDC error query. Trailing slash and
  // any case, matching vue-router; /login/extra and /logins stay React.
  /^\/login\/?$/i,
];

export function isVueAppPath(pathname: string): boolean {
  return VUE_APP_PATHS.some((pattern) => pattern.test(pathname));
}
