import { t } from "@fvoci/i18n";
import type { WorkspaceRole } from "@/lib/contracts";

const ORDER = ["guest", "member", "admin", "owner"] as const;

export const WORKSPACE_ROLES: WorkspaceRole[] = ["owner", "admin", "member", "guest"];

/** Workspace role rank used by the React settings page. */
export function roleAtLeast(role: string, minimum: string): boolean {
  return (
    ORDER.indexOf(role as (typeof ORDER)[number]) >=
    ORDER.indexOf(minimum as (typeof ORDER)[number])
  );
}

export function roleLabel(role: string): string {
  if (role === "owner") return t("role.owner");
  if (role === "admin") return t("role.admin");
  if (role === "guest") return t("role.guest");
  return t("role.member");
}

/** Roles the current user may assign (at or below their own rank). */
export function inviteRolesFor(currentUserRole: string): WorkspaceRole[] {
  return WORKSPACE_ROLES.filter((role) => roleAtLeast(currentUserRole, role));
}

export function canManageMember(input: {
  currentUserRole: string;
  currentUserId: string | null;
  memberUserId: string;
  memberRole: string;
}): boolean {
  return (
    roleAtLeast(input.currentUserRole, "admin") &&
    input.memberUserId !== input.currentUserId &&
    roleAtLeast(input.currentUserRole, input.memberRole)
  );
}
