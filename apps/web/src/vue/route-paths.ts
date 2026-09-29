/**
 * The Vue app's route paths. src/app-boundary.ts must send exactly these
 * paths to the Vue app (src/app-boundary.test.ts checks both agree).
 *
 * Workspace settings routes are declared here so the Vue app can render them
 * once the coordinator-owned boundary moves. Until then boot still loads React
 * for `/w/:slug/settings` (and nested document-tags/templates).
 */
export const VUE_ROUTE_PATHS = {
  projectGantt: "/w/:slug/:ref/gantt",
  // The wiki document refs of lib/href.ts parseWikiRef; route paths match
  // case-insensitively, as the boundary does.
  wikiDocument: "/w/:slug/:ref(wiki-[1-9]\\d{0,8})",
  workspaceSettings: "/w/:slug/settings",
  documentTagsSettings: "/w/:slug/settings/document-tags",
  templatesSettings: "/w/:slug/settings/templates",
} as const;
