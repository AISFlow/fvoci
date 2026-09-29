// The temporary React/Vue boundary. One index.html serves every SPA path (the
// server's fallback and head injection only know that file); src/boot.ts loads
// the Vue app for the paths below and the React app for everything else.
// Crossing the boundary is always a full page load: the two apps share no
// runtime objects, only the session cookie and the framework-neutral modules.
// A path moves here when its Vue flow is accepted and its React page is removed.

/** Path patterns the Vue app renders (src/vue/router.ts declares the same routes). */
export const VUE_APP_PATHS: readonly RegExp[] = [
  // Project Gantt: /w/:slug/:ref/gantt
  /^\/w\/[^/]+\/[^/]+\/gantt\/?$/,
];

export function isVueAppPath(pathname: string): boolean {
  return VUE_APP_PATHS.some((pattern) => pattern.test(pathname));
}
