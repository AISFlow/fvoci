import { createRouter, createWebHistory, type RouteRecordRaw, type RouterHistory } from "vue-router";
import { isVueAppPath } from "@/app-boundary";
import ProjectGanttPage from "./pages/ProjectGanttPage.vue";
import { VUE_ROUTE_PATHS } from "./route-paths";

/** The Vue app's pages; src/app-boundary.ts sends exactly these paths here. */
export const routes: RouteRecordRaw[] = [
  { path: VUE_ROUTE_PATHS.projectGantt, name: "project-gantt", component: ProjectGanttPage },
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
  // boot module loads the React app for it.
  router.beforeEach((to) => {
    if (isVueAppPath(to.path)) return true;
    window.location.assign(to.fullPath);
    return false;
  });
  return router;
}
