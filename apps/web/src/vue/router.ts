import {
  createRouter,
  createWebHistory,
  stringifyQuery,
  type RouteRecordRaw,
  type RouterHistory,
} from "vue-router";
import {
  VUE_SETTINGS_ROUTE_PATHS,
  VUE_ACCOUNT_ROUTE_PATHS,
  VUE_NAV_ROUTE_PATHS,
  VUE_ROUTE_PATHS,
  VUE_WORKSPACE_ROUTE_PATHS,
} from "./route-paths";

/** Each page is a lazy chunk, so Gantt and the app entry do not load the
 * document editor or its styles (import-graph and browser groups verify it). */
export const routes: RouteRecordRaw[] = [
  {
    path: VUE_ROUTE_PATHS.projectFields,
    name: "project-fields",
    component: () => import("./features/projects/settings/ProjectFieldsPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.projectWorkflow,
    name: "project-workflow",
    component: () => import("./features/projects/settings/ProjectWorkflowPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.projectGantt,
    name: "project-gantt",
    component: () => import("./pages/ProjectGanttPage.vue"),
  },
  // Wiki refs are more specific than project home's `/w/:slug/:ref` and must
  // stay listed first so `/w/acme/wiki-3` is never the project overview.
  {
    path: VUE_ROUTE_PATHS.wikiDocument,
    name: "wiki-document",
    component: () => import("./pages/WikiDocumentPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.projectTasks,
    name: "project-tasks",
    component: () => import("./pages/ProjectTasksPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.projectTable,
    name: "project-table",
    component: () => import("./pages/ProjectCollectionPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.projectBoard,
    name: "project-board",
    component: () => import("./pages/ProjectCollectionPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.projectCalendar,
    name: "project-calendar",
    component: () => import("./pages/ProjectCollectionPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.workspaceItem,
    name: "workspace-item",
    component: () => import("./pages/WorkspaceItemPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.projectHome,
    name: "project-home",
    component: () => import("./pages/ProjectHomePage.vue"),
  },
  { path: VUE_ROUTE_PATHS.login, name: "login", component: () => import("./pages/LoginPage.vue") },
  {
    path: VUE_ROUTE_PATHS.resetPassword,
    name: "reset-password",
    component: () => import("./pages/ResetPasswordPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.magicLink,
    name: "magic-link",
    component: () => import("./pages/MagicLinkPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.confirmEmail,
    name: "confirm-email",
    component: () => import("./pages/ConfirmEmailPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.cancelWithdraw,
    name: "cancel-withdraw",
    component: () => import("./pages/CancelWithdrawPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.consent,
    name: "consent",
    component: () => import("./pages/ConsentPage.vue"),
  },
  { path: VUE_ROUTE_PATHS.home, name: "home", component: () => import("./pages/HomePage.vue") },
  { path: VUE_ROUTE_PATHS.legal, name: "legal", component: () => import("./pages/LegalPage.vue") },
  {
    path: VUE_ROUTE_PATHS.serviceInfo,
    name: "service-info",
    component: () => import("./pages/ServiceInfoPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.invite,
    name: "invite",
    component: () => import("./pages/InvitePage.vue"),
  },
  { path: VUE_ROUTE_PATHS.setup, name: "setup", component: () => import("./pages/SetupPage.vue") },
  // Exact section routes stay separate from project and item refs.
  {
    path: VUE_WORKSPACE_ROUTE_PATHS.projects,
    name: "projects",
    component: () => import("./pages/ProjectsPage.vue"),
  },
  {
    path: VUE_WORKSPACE_ROUTE_PATHS.wikiList,
    name: "wiki-list",
    component: () => import("./pages/WikiPage.vue"),
  },
  {
    path: VUE_WORKSPACE_ROUTE_PATHS.search,
    name: "search",
    component: () => import("./pages/SearchPage.vue"),
  },

  {
    path: VUE_NAV_ROUTE_PATHS.myTasks,
    name: "my-tasks",
    component: () => import("./pages/MyTasksPage.vue"),
  },
  {
    path: VUE_NAV_ROUTE_PATHS.notifications,
    name: "notifications",
    component: () => import("./pages/NotificationsPage.vue"),
  },
  {
    path: VUE_NAV_ROUTE_PATHS.trash,
    name: "trash",
    component: () => import("./pages/TrashPage.vue"),
  },
  {
    path: VUE_WORKSPACE_ROUTE_PATHS.workspaceHome,
    name: "workspace-home",
    component: () => import("./pages/WorkspaceHomePage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.attachmentView,
    name: "attachment-view",
    component: () => import("./pages/AttachmentViewPage.vue"),
  },
  {
    path: VUE_ROUTE_PATHS.shareAttachmentView,
    name: "share-attachment-view",
    component: () => import("./pages/ShareAttachmentViewPage.vue"),
  },
  {
    path: VUE_ACCOUNT_ROUTE_PATHS.accountSettings,
    name: "account-settings",
    component: () => import("./pages/AccountSettingsPage.vue"),
  },
  {
    path: VUE_ACCOUNT_ROUTE_PATHS.admin,
    name: "admin",
    component: () => import("./pages/AdminPage.vue"),
  },
  {
    path: VUE_ACCOUNT_ROUTE_PATHS.adminAudit,
    name: "admin-audit",
    component: () => import("./pages/AdminAuditPage.vue"),
  },
  {
    path: VUE_ACCOUNT_ROUTE_PATHS.adminLegal,
    name: "admin-legal",
    component: () => import("./pages/AdminLegalPage.vue"),
  },
  {
    path: VUE_SETTINGS_ROUTE_PATHS.documentTagsSettings,
    name: "workspace-settings-document-tags",
    component: () => import("./pages/DocumentTagsSettingsPage.vue"),
  },
  {
    path: VUE_SETTINGS_ROUTE_PATHS.templatesSettings,
    name: "workspace-settings-templates",
    component: () => import("./pages/TemplatesSettingsPage.vue"),
  },
  {
    path: VUE_SETTINGS_ROUTE_PATHS.workspaceSettings,
    name: "workspace-settings",
    component: () => import("./pages/WorkspaceSettingsPage.vue"),
  },
  // Public share is an anonymous reader with its own session-free gate.
  {
    path: VUE_ROUTE_PATHS.publicShare,
    name: "public-share",
    component: () => import("./pages/PublicSharePage.vue"),
  },
  // Decoded refs outside the raw route grammar canonicalize after the same
  // workspace/session gates as resource pages. Invalid refs keep the shell.
  {
    path: VUE_ROUTE_PATHS.workspaceRef,
    name: "workspace-ref",
    component: () => import("./pages/WorkspaceRefPage.vue"),
  },
];

export function createAppRouter(history: RouterHistory = createWebHistory()) {
  const router = createRouter({
    history,
    // Reusing the current query during a canonical ref replacement preserves
    // its original spelling (including repeated keys and encoded spaces).
    // New query objects use Vue Router's standard serialization.
    stringifyQuery: (query) => {
      const current = router.currentRoute.value;
      if (query !== current.query) return stringifyQuery(query);
      const suffix = current.fullPath.slice(current.path.length);
      return suffix.startsWith("?") ? suffix.slice(1).split("#", 1)[0]! : "";
    },
    routes: [...routes, { path: "/:pathMatch(.*)*", name: "unknown-path", redirect: "/" }],
  });
  return router;
}
