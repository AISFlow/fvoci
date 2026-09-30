/**
 * The Vue app's route paths. src/app-boundary.ts must send exactly these
 * paths to the Vue app (src/app-boundary.test.ts checks both agree).
 *
 * Workspace landing `/w/:slug` and the project list `/w/:slug/projects` are
 * implemented (VUE_WORKSPACE_ROUTE_PATHS) but stay off this object until
 * the coordinator adds the matching regexes to app-boundary.ts:
 *   /^\/w\/[^/]+\/?$/i
 *   /^\/w\/[^/]+\/projects\/?$/i
 * Fold them into this object in the same change as those regexes, and extend
 * app-boundary.test.ts SAMPLES (`/w/acme`, `/w/acme/projects`, trailing slashes
 * and mixed case).
 *
 * Wiki list `/w/:slug/wiki` and workspace search `/w/:slug/search` are also
 * implemented here and stay off this object until:
 *   /^\/w\/[^/]+\/wiki\/?$/i
 *   /^\/w\/[^/]+\/search\/?$/i
 * Those must not swallow wiki documents (`/w/:slug/WIKI-<n>`), which already
 * boot the Vue app. Settings, my-tasks and the rest stay React pages.
 */
// Escape inner closing parentheses for vue-router's custom-regexp parser.
// Reserved workspace screens and item refs must never resolve as project home.
const projectRef = "(?!(?:projects|search|wiki|trash|my-tasks|notifications|settings|a)(?:/|$))(?![^/]*-\\d+(?:/|$))[A-Za-z][A-Za-z0-9-]{1,31}";

export const VUE_ROUTE_PATHS = {
  projectHome: `/w/:slug/:ref(${projectRef.replaceAll(")", "\\)")})`,
  workspaceItem: "/w/:slug/:ref([A-Za-z0-9-]{2,32}-[1-9]\\d{0,8})",
  projectTasks: "/w/:slug/:ref/tasks",
  projectTable: "/w/:slug/:ref/table",
  projectBoard: "/w/:slug/:ref/board",
  projectCalendar: "/w/:slug/:ref/calendar",
  projectGantt: "/w/:slug/:ref/gantt",
  // The wiki document refs of lib/href.ts parseWikiRef; route paths match
  // case-insensitively, as the boundary does.
  wikiDocument: "/w/:slug/:ref(wiki-[1-9]\\d{0,8})",
  login: "/login",
  resetPassword: "/reset-password",
  magicLink: "/magic-link",
  confirmEmail: "/confirm-email",
  cancelWithdraw: "/cancel-withdraw",
  consent: "/consent",
  home: "/",
  legal: "/legal/:kind",
  serviceInfo: "/service-info",
  invite: "/invite/:token",
  setup: "/setup",
  // Session attachment viewer: /w/:slug/a/:attachmentId/view
  attachmentView: "/w/:slug/a/:attachmentId/view",
  // Anonymous share attachment viewer: /s/:token/attachments/:attachmentId/view
  shareAttachmentView: "/s/:token/attachments/:attachmentId/view",
} as const;

/**
 * Vue routes for the workspace landing, project list, wiki list and search.
 * Boot still uses app-boundary.ts, so these pages load only after the regexes
 * above land. Leaving React until then means a full page load from a Vue page
 * (Gantt, wiki document) still opens the existing React home/list/wiki/search.
 */
export const VUE_WORKSPACE_ROUTE_PATHS = {
  workspaceHome: "/w/:slug",
  projects: "/w/:slug/projects",
  wikiList: "/w/:slug/wiki",
  search: "/w/:slug/search",
} as const;

/**
 * Vue pages for workspace nav (my-tasks, notifications, trash). Declared as
 * lazy chunks so the Vue app can render them once boot moves; until then
 * src/app-boundary.ts still sends these paths to React, so afterEach full-loads.
 * Coordinator regexes to add later (do not put these on VUE_ROUTE_PATHS yet):
 *   /^\/w\/[^/]+\/my-tasks\/?$/i
 *   /^\/w\/[^/]+\/notifications\/?$/i
 *   /^\/w\/[^/]+\/trash\/?$/i
 */
export const VUE_NAV_ROUTE_PATHS = {
  myTasks: "/w/:slug/my-tasks",
  notifications: "/w/:slug/notifications",
  trash: "/w/:slug/trash",
} as const;

/** Staged account/admin pages; boot remains React until their owner verifies them. */
export const VUE_ACCOUNT_ROUTE_PATHS = {
  accountSettings: "/settings/account",
  admin: "/settings/admin",
  adminAudit: "/settings/audit",
  adminLegal: "/settings/legal",
} as const;
