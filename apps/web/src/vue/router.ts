import { createRouter, createWebHistory, type RouteRecordRaw, type RouterHistory } from "vue-router";
import { isVueAppPath } from "@/app-boundary";
import { STAGED_VUE_ROUTE_PATHS, VUE_ROUTE_PATHS } from "./route-paths";

/** The Vue app's pages; src/app-boundary.ts sends exactly these paths here.
 * Each page is its own chunk, so the Gantt page does not load the wiki
 * editor (Tiptap, Yjs, the collab provider) or its stylesheets
 * (import-graph.test.ts and e2e/project-gantt-flow.spec.ts check this). */
export const routes: RouteRecordRaw[] = [
  { path: VUE_ROUTE_PATHS.projectGantt, name: "project-gantt", component: () => import("./pages/ProjectGanttPage.vue") },
  // Wiki refs are more specific than project home's `/w/:slug/:ref` and must
  // stay listed first so `/w/acme/wiki-3` is never the project overview.
  { path: VUE_ROUTE_PATHS.wikiDocument, name: "wiki-document", component: () => import("./pages/WikiDocumentPage.vue") },
  // Task list / collection views. src/app-boundary.ts is coordinator-owned;
  // these routes stay unreachable until that regex list includes them
  // (VUE_ROUTE_PATHS must be updated in the same change). A navigation that
  // reaches them while the boundary still sends React will full-load away
  // (afterEach below).
  { path: "/w/:slug/:ref/tasks", name: "project-tasks", component: () => import("./pages/ProjectTasksPage.vue") },
  { path: "/w/:slug/:ref/table", name: "project-table", component: () => import("./pages/ProjectCollectionPage.vue") },
  { path: "/w/:slug/:ref/board", name: "project-board", component: () => import("./pages/ProjectCollectionPage.vue") },
  { path: "/w/:slug/:ref/calendar", name: "project-calendar", component: () => import("./pages/ProjectCollectionPage.vue") },
  // Task and project-document item refs. More specific than project-home's
  // `/w/:slug/:ref`; wiki-document above is more specific still (`wiki-3`
  // never reaches this page). Not live: STAGED_WORKSPACE_ITEM_PATH is the
  // regex the coordinator would add later.
  {
    path: STAGED_VUE_ROUTE_PATHS.workspaceItem,
    name: "workspace-item",
    component: () => import("./pages/WorkspaceItemPage.vue"),
  },
  // Project home overview. Same `/w/:slug/:ref` shape as the collection
  // routes; wiki-document and workspace-item above are more specific. Not
  // live: STAGED_PROJECT_HOME_PATH is the regex the coordinator would add later.
  {
    path: STAGED_VUE_ROUTE_PATHS.projectHome,
    name: "project-home",
    component: () => import("./pages/ProjectHomePage.vue"),
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
