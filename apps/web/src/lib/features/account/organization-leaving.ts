/**
 * Leaving an organization from the settings: which organizations the reader
 * can leave, by the names they know them by, and what the dialog says of a
 * refusal. The words come from the refusal's status and reason, never from
 * the server's detail.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError, type UserOrganization } from '$lib/state/index.ts';

/** An organization the reader is a member of, by its own name. */
export interface LeavableOrganization {
  orgId: string;
  name: string;
}

/**
 * The organizations that people work in together that the reader is a member
 * of, from `GET /api/v1/users/me/organizations` (including ones with no
 * workspaces left).
 */
export function leavableOrganizations(
  organizations: readonly UserOrganization[],
): LeavableOrganization[] {
  return organizations.map((organization) => ({
    orgId: organization.id,
    name: organization.name || m.account_delete_organization_unnamed(),
  }));
}

export interface LeaveProblem {
  text: string;
  /** Asking again changes nothing until the reader does what the text says. */
  settled: boolean;
}

export function leaveProblemOf(error: unknown): LeaveProblem {
  if (!isBackendError(error)) return { text: m.account_failed(), settled: false };
  if (error.status === 404) return { text: m.org_leave_not_member(), settled: true };
  switch (error.reason) {
    case 'password_required':
      return { text: m.org_leave_password_required(), settled: true };
    case 'last_instance_admin':
      return { text: m.org_leave_last_instance_admin(), settled: true };
    case 'sole_org_admin':
      return { text: m.org_leave_sole_admin(), settled: true };
    case 'alone_in_organization':
      return { text: m.org_leave_alone(), settled: true };
    default:
      return { text: m.account_failed(), settled: false };
  }
}
