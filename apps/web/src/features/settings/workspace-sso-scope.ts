// Workspace SSO belongs to team workspaces. Anyone can create a personal
// workspace and would choose its IdP, so the server refuses to save SSO there
// (409 personal_workspace_is_immutable) and a row saved before that rule
// starts no sign-in.

/** Whether the workspace settings page shows the SSO section. */
export function showsWorkspaceSso(kind: string, canManage: boolean): boolean {
  return canManage && kind === "team";
}
