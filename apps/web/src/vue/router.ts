import { createRouter, createWebHistory, type RouteRecordRaw, type RouterHistory } from "vue-router";
import { isVueAppPath } from "@/app-boundary";
import ProjectGanttPage from "./pages/ProjectGanttPage.vue";
import WikiDocumentPage from "./pages/WikiDocumentPage.vue";
import { VUE_ROUTE_PATHS } from "./route-paths";

/** The Vue app's pages; src/app-boundary.ts sends exactly these paths here. */
export const routes: RouteRecordRaw[] = [
  { path: VUE_ROUTE_PATHS.projectGantt, name: "project-gantt", component: ProjectGanttPage },
  { path: VUE_ROUTE_PATHS.wikiDocument, name: "wiki-document", component: WikiDocumentPage },
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
  // vue-router undo a history pop with go(-1), which races the load.
  router.afterEach((to) => {
    if (!isVueAppPath(to.path)) window.location.replace(to.fullPath);
  });
  return router;
}
