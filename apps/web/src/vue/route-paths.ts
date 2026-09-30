/**
 * The Vue app's route paths. src/app-boundary.ts must send exactly these
 * paths to the Vue app (src/app-boundary.test.ts checks both agree).
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
} as const;
