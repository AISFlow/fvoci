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
export const PROJECT_HOME_PATH =
  /^\/w\/[^/]+\/(?!(?:projects|search|wiki|trash|my-tasks|notifications|settings|a)\/?$)(?![^/]*-\d+\/?$)[A-Za-z][A-Za-z0-9-]{1,31}\/?$/i;
export const WORKSPACE_ITEM_PATH =
  /^\/w\/[^/]+\/(?!wiki-[1-9]\d{0,8}\/?$)[A-Za-z0-9-]{2,32}-[1-9]\d{0,8}\/?$/i;

export const VUE_APP_PATHS: readonly RegExp[] = [
  // Workspace entrance and section lists, with no nested-path ownership.
  /^\/w\/[^/]+\/?$/i,
  /^\/w\/[^/]+\/(?:projects|wiki|search|my-tasks|notifications|trash)\/?$/i,
  // Home workspace picker and public policies/operator information.
  /^\/$/,
  /^\/legal\/[^/]+\/?$/i,
  /^\/service-info\/?$/i,
  // Account and instance administration; nested paths stay outside ownership.
  /^\/settings\/(?:account|admin|audit|legal)\/?$/i,
  // Workspace settings and its exact tag/template screens.
  /^\/w\/[^/]+\/settings(?:\/(?:document-tags|templates))?\/?$/i,
  // Existing project overview, task/document items and collection views.
  PROJECT_HOME_PATH,
  WORKSPACE_ITEM_PATH,
  /^\/w\/[^/]+\/[^/]+\/(?:tasks|table|board|calendar)\/?$/i,
  // Existing project workflow and collection-field settings.
  /^\/w\/[^/]+\/[^/]+\/settings\/(?:fields|workflow)\/?$/i,
  // Project Gantt: /w/:slug/:ref/gantt
  /^\/w\/[^/]+\/[^/]+\/gantt\/?$/i,
  // Wiki document: /w/:slug/WIKI-<n>, the refs parseWikiRef (lib/href.ts)
  // accepts (1-9 digits, no leading zero, prefix in any case). Other spellings
  // that the React router decodes to one of these (percent-encoded) reach
  // WorkspaceRefPage, which reloads the canonical path.
  /^\/w\/[^/]+\/wiki-[1-9]\d{0,8}\/?$/i,
  // Auth links consume tokens only after an explicit submission.
  /^\/reset-password\/?$/i,
  /^\/magic-link\/?$/i,
  /^\/confirm-email\/?$/i,
  /^\/cancel-withdraw\/?$/i,
  /^\/consent\/?$/i,
  // Login: logout landing, MFA step, OIDC error query. Trailing slash and
  // any case, matching vue-router; /login/extra and /logins stay React.
  /^\/login\/?$/i,
  // Public invite: token is exactly one segment; nested paths stay React.
  /^\/invite\/[^/]+\/?$/i,
  // First-instance setup. /setup/extra and /setups stay React.
  /^\/setup\/?$/i,
  // Anonymous public share reader; nested attachment paths retain their viewer.
  /^\/s\/[^/]+\/?$/i,
  // Session and anonymous share attachment viewers.
  /^\/w\/[^/]+\/a\/[^/]+\/view\/?$/i,
  /^\/s\/[^/]+\/attachments\/[^/]+\/view\/?$/i,
];

export function isVueAppPath(pathname: string): boolean {
  return VUE_APP_PATHS.some((pattern) => pattern.test(pathname));
}
