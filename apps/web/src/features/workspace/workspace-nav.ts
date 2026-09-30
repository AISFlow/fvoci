import {
  myTasksPath,
  notificationsPath,
  projectsPath,
  searchPath,
  settingsPath,
  wikiPath,
  workspaceHomePath,
} from "@/lib/href";

/** The workspace header section a page belongs to (both web apps' workspace shells). */
export type WorkspaceNav =
  | "home"
  | "wiki"
  | "settings"
  | "projects"
  | "myTasks"
  | "search"
  | "notifications";

/** Where switching to another workspace lands: the same section there. */
export function landingPath(slug: string, activeNav: WorkspaceNav): string {
  if (activeNav === "settings") return settingsPath(slug);
  if (activeNav === "projects") return projectsPath(slug);
  if (activeNav === "myTasks") return myTasksPath(slug);
  if (activeNav === "search") return searchPath(slug);
  if (activeNav === "notifications") return notificationsPath(slug);
  if (activeNav === "home") return workspaceHomePath(slug);
  return wikiPath(slug);
}
