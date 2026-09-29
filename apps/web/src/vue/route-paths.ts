/**
 * The Vue app's route paths. src/app-boundary.ts must send exactly these
 * paths to the Vue app (src/app-boundary.test.ts checks both agree).
 * Coordinator-owned regex still needed for the attachment viewers:
 *   /^\/w\/[^/]+\/a\/[^/]+\/view\/?$/i
 *   /^\/s\/[^/]+\/attachments\/[^/]+\/view\/?$/i
 */
export const VUE_ROUTE_PATHS = {
  projectGantt: "/w/:slug/:ref/gantt",
  // The wiki document refs of lib/href.ts parseWikiRef; route paths match
  // case-insensitively, as the boundary does.
  wikiDocument: "/w/:slug/:ref(wiki-[1-9]\\d{0,8})",
  // Session attachment viewer: /w/:slug/a/:attachmentId/view
  attachmentView: "/w/:slug/a/:attachmentId/view",
  // Anonymous share attachment viewer: /s/:token/attachments/:attachmentId/view
  shareAttachmentView: "/s/:token/attachments/:attachmentId/view",
} as const;
