/** Live paths agree with app-boundary.ts; staged settings/account pages stay React. */
// Escape inner closing parentheses for vue-router's custom-regexp parser.
// Reserved workspace screens and item refs must never resolve as project home.
const projectRef = "(?!(?:projects|search|wiki|trash|my-tasks|notifications|settings|a)(?:/|$))(?![^/]*-\\d+(?:/|$))[A-Za-z][A-Za-z0-9-]{1,31}";

const VUE_BASE_ROUTE_PATHS = {
  projectFields: "/w/:slug/:ref/settings/fields",
  projectWorkflow: "/w/:slug/:ref/settings/workflow",
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
  publicShare: "/s/:token",
  shareAttachmentView: "/s/:token/attachments/:attachmentId/view",
} as const;

/** Live workspace entrance and section routes. */
export const VUE_WORKSPACE_ROUTE_PATHS = {
  workspaceHome: "/w/:slug",
  projects: "/w/:slug/projects",
  wikiList: "/w/:slug/wiki",
  search: "/w/:slug/search",
} as const;

/** Live personal workspace navigation. */
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

/** Connected workspace settings paths. */
export const VUE_SETTINGS_ROUTE_PATHS = {
  workspaceSettings: "/w/:slug/settings",
  documentTagsSettings: "/w/:slug/settings/document-tags",
  templatesSettings: "/w/:slug/settings/templates",
} as const;

export const VUE_ROUTE_PATHS = {
  ...VUE_BASE_ROUTE_PATHS,
  ...VUE_WORKSPACE_ROUTE_PATHS,
  ...VUE_NAV_ROUTE_PATHS,
  ...VUE_SETTINGS_ROUTE_PATHS,
} as const;
