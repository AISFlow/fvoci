import { createRouter, createWebHistory, type RouteRecordRaw, type RouterHistory } from "vue-router";
import { isVueAppPath } from "@/app-boundary";
import { VUE_NAV_ROUTE_PATHS, VUE_ROUTE_PATHS, VUE_WORKSPACE_ROUTE_PATHS } from "./route-paths";

/** The Vue app's pages; src/app-boundary.ts sends exactly these paths here.
 * Each page is its own chunk, so the Gantt page does not load the wiki
 * editor (Tiptap, Yjs, the collab provider) or its stylesheets
 * (import-graph.test.ts and e2e/project-gantt-flow.spec.ts check this). */
export const routes: RouteRecordRaw[] = [
  { path: VUE_ROUTE_PATHS.projectGantt, name: "project-gantt", component: () => import("./pages/ProjectGanttPage.vue") },
  { path: VUE_ROUTE_PATHS.wikiDocument, name: "wiki-document", component: () => import("./pages/WikiDocumentPage.vue") },
  // More specific than `/w/:slug`. Coordinator-owned app-boundary.ts still
  // boots the React app for these until its regexes include them.
  { path: VUE_WORKSPACE_ROUTE_PATHS.projects, name: "projects", component: () => import("./pages/ProjectsPage.vue") },
  // More specific than `/w/:slug`. Boot still loads React until the
  // coordinator regexes in route-paths.ts (VUE_NAV_ROUTE_PATHS) land.
  { path: VUE_NAV_ROUTE_PATHS.myTasks, name: "my-tasks", component: () => import("./pages/MyTasksPage.vue") },
  {
    path: VUE_NAV_ROUTE_PATHS.notifications,
    name: "notifications",
    component: () => import("./pages/NotificationsPage.vue"),
  },
  { path: VUE_NAV_ROUTE_PATHS.trash, name: "trash", component: () => import("./pages/TrashPage.vue") },
  {
    path: VUE_WORKSPACE_ROUTE_PATHS.workspaceHome,
    name: "workspace-home",
    component: () => import("./pages/WorkspaceHomePage.vue"),
  },
];

export function createAppRouter(history: RouterHistory = createWebHistory()) {
  const router = createRouter({
    history,
    routes: [
      ...routes,
      // Never rendered: a path outside the Vue app is left by a full load.
      { path: "/:pathMatch(.*)*", name: "react-app", component: { render: () => null } },
    ],
  });
  // Any in-app navigation to a React page becomes a full page load, so the
  // boot module loads the React app for it. The navigation completes first
  // (the Vue page unmounts, its collab room flushes and closes) and the load
  // replaces the entry it made. A guard that cancelled it instead would make
  // vue-router undo a history pop with go(-1), which races the load. A
  // navigation that failed or was superseded by another one did not happen:
  // afterEach sees those too, and they load nothing.
  router.afterEach((to, _from, failure) => {
    if (!failure && !isVueAppPath(to.path)) window.location.replace(to.fullPath);
  });
  return router;
}
