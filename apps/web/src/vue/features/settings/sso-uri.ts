/** `{origin}/api/v1/auth/sso/{workspaceId}/callback`, as the server builds it. */
export function workspaceSsoRedirectUri(origin: string, workspaceId: string): string {
  const base = origin.replace(/\/+$/, "");
  return `${base}/api/v1/auth/sso/${encodeURIComponent(workspaceId)}/callback`;
}

/**
 * The URI to show: the server's own, since the provider compares it as an
 * exact string; built from `origin` only when the server sent none.
 */
export function displayedRedirectUri(
  serverUri: string | null | undefined,
  origin: string,
  workspaceId: string,
): string {
  return serverUri ? serverUri : workspaceSsoRedirectUri(origin, workspaceId);
}
