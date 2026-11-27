/**
 * Deleting the account: what the form says about a refusal, and at which
 * field, and how long an account that waits still has. The words come from
 * the refusal's status and reason, never from the server's detail.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isBackendError, type LeavingOrganization } from '$lib/state/index.ts';

export interface DeletionProblem {
  text: string;
  /** The field to point at, if the refusal is about one. */
  field: 'username' | 'password' | undefined;
}

export function deletionProblemOf(error: unknown): DeletionProblem {
  if (!isBackendError(error)) return { text: m.account_failed(), field: undefined };
  if (error.reason === 'invalid_credentials') {
    return { text: m.account_password_wrong(), field: 'password' };
  }
  if (error.status === 429) return { text: m.account_rate_limited(), field: undefined };
  if (error.status === 400 && error.refusalAt('/username')) return wrongName();
  switch (error.reason) {
    case 'sign_in_again':
      return { text: m.account_delete_sign_in_again(), field: undefined };
    case 'sso_required':
      return { text: m.account_delete_sso_required(), field: undefined };
    case 'last_admin':
      return { text: m.account_delete_last_instance_admin(), field: undefined };
    case 'last_org_admin':
      return { text: m.account_delete_last_org_admin(), field: undefined };
    default:
      return { text: m.account_failed(), field: undefined };
  }
}

/** The name typed is not the account's: said before the server is asked, and when it says so. */
export function wrongName(): DeletionProblem {
  return { text: m.account_delete_wrong_username(), field: 'username' };
}

/** Whether `typed` names the account, as the server compares: in any letter case. */
export function namesAccount(typed: string, username: string): boolean {
  return typed.trim().toLowerCase() === username.toLowerCase();
}

const DAY = 24 * 60 * 60 * 1000;

/** The days left until `deleteAfter`, a day that has begun counted whole; 0 once it has passed. */
export function daysLeft(deleteAfter: number, now: number): number {
  return Math.max(0, Math.ceil((deleteAfter - now) / DAY));
}

/**
 * The account does not wait any more, when that is what a refusal of the
 * waiting screen says: the deletion was taken back elsewhere (409), or the
 * session is over, as it is once the account is erased (a 401 that is not
 * the waiting account's own). The page starts again to see which.
 */
export function waitsNoMore(error: unknown): boolean {
  if (!isBackendError(error)) return false;
  return (
    error.status === 409 || (error.status === 401 && error.reason !== 'account_deletion_pending')
  );
}

/**
 * What goes with the account, the reader's own space apart (`own`, which the
 * server always names) from the organizations of people that have nobody else.
 */
export function leavingOf(organizations: readonly LeavingOrganization[]): {
  own: LeavingOrganization | undefined;
  alone: LeavingOrganization[];
} {
  return {
    own: organizations.find((organization) => organization.kind === 'personal'),
    alone: organizations.filter((organization) => organization.kind !== 'personal'),
  };
}

/**
 * How much is in an organization that goes with the account: "3 boards, 1
 * wiki, 12 files", and the diaries of the own space, the only one with any.
 */
export function holdingsOf(organization: LeavingOrganization): {
  boards: string;
  wikis: string;
  diaries: string;
  files: string;
} {
  const [boards, wikis, diaries, files] = [
    organization.boards ?? 0,
    organization.wikis ?? 0,
    organization.diaries ?? 0,
    organization.files ?? 0,
  ];
  return {
    boards:
      boards === 1 ? m.account_delete_boards_one() : m.account_delete_boards({ count: boards }),
    wikis: wikis === 1 ? m.account_delete_wikis_one() : m.account_delete_wikis({ count: wikis }),
    diaries:
      diaries === 1 ? m.account_delete_diaries_one() : m.account_delete_diaries({ count: diaries }),
    files: files === 1 ? m.account_delete_files_one() : m.account_delete_files({ count: files }),
  };
}
