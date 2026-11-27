/** A workspace of the reader's, as their list has it (part of `backend.ts`). */
import type { WorkspaceRole } from './people-types.ts';
import type { BackendError } from './problems.ts';

/**
 * What the organization of a workspace is: `personal` for the reader's own
 * space ("My space"), which is theirs alone and has nobody to administer it;
 * for every other, the kind of an organization that people work in together.
 */
export type OrgKind = 'shared' | 'personal';

/** What a member of an organization is in it. */
export type OrgRole = 'admin' | 'member';

/** A shared organization the reader is a member of (`GET /users/me/organizations`). */
export interface UserOrganization {
  id: string;
  /** Empty if it was given none. */
  name: string;
  role: OrgRole;
}

export interface WorkspaceInfo {
  id: string;
  /** Empty if it was given none, as the reader's own space always is: a screen names that one. */
  title: string;
  /** The organization the workspace belongs to, where the backend has organizations (the server). */
  orgId?: string;
  /** What that organization is; left out by a backend that does not say, as for one of people. */
  kind?: OrgKind;
  /**
   * The workspace is closed to this session: its organization takes no
   * password, and it opens for a session made through the provider of `orgId`.
   */
  ssoRequired?: boolean;
  /**
   * The workspace is closed to this session: it is the reader's own space, the
   * account has a password, and the session was not made with it (but through
   * a provider). Signing in with the password opens it.
   */
  passwordRequired?: boolean;
  /**
   * What the reader is in it; left out by a backend that has no roles, where
   * the reader does everything. A guest is on the boards they were invited
   * to, and makes none.
   */
  role?: WorkspaceRole;
}

/**
 * Whether the workspace is the reader's own space: nobody else is in it, so
 * there is nobody to invite, no group of people and no organization to administer;
 * what is in it is shown to others by a public link alone.
 */
export const isOwnSpace = (workspace: WorkspaceInfo | undefined): boolean =>
  workspace?.kind === 'personal';

/** `signedOut`: nobody is signed in (401); `leaving`: the account waits to be deleted (its session opens only taking that back). */
export type LoadStatus = 'idle' | 'loading' | 'ready' | 'failed' | 'signedOut' | 'leaving';

/** What failed in the workspace, as a key the shell has a message for. */
export type ProblemAction =
  'load' | 'save' | 'undo' | 'export' | 'import' | 'movePage' | 'moveUnit';

export interface Problem {
  id: number;
  action: ProblemAction;
  error: BackendError | undefined;
}
