/**
 * What an organization's administrators manage (its workspaces, its members'
 * roles, its public links) and the audit log of a workspace, which its members
 * read. A boundary of its own beside `Backend`: only the server has it. The
 * backend in memory has no organization, and the screens that need one are
 * not offered there.
 */
import type { PersonName } from '../logic/person-names.ts';
import type { OrgRole } from './workspace-types.ts';

export type { OrgRole };

/** What somebody is in a workspace, as the organization lists it. */
export type OrgWorkspaceRole = 'admin' | 'member' | 'guest';

export interface Organization {
  id: string;
  /** Empty when it was given none. */
  name: string;
  /** Whether its boards may be read by a public link. */
  publicLinks: boolean;
  /** Whether members may copy containers out of this organization. */
  allowCopyOut: boolean;
  /**
   * Whether members may give its workspaces to personal API tokens while it
   * takes only sessions made through its provider; switched off, such tokens
   * open nothing of it.
   */
  allowApiTokens: boolean;
  /** The workspace whoever joins the organization gets into; it is not deleted. */
  defaultWorkspaceId: string;
}

export interface OrgMember {
  userId: string;
  username: string;
  displayName: string;
  /** The parts of the name, the title and the credentials, where they were told. */
  name?: PersonName;
  role: OrgRole;
}

/** A workspace as the organization's administrators list it: nothing of what is in it. */
export interface OrgWorkspace {
  id: string;
  name: string;
  memberCount: number;
  isDefault: boolean;
  /** When it was put in the trash, in milliseconds; omitted while it is in use. */
  deletedAt?: number;
  /** What the administrator who asks is in it; null for no member. */
  role: OrgWorkspaceRole | null;
  /** Whether they are in it having joined it as the organization's administrator. */
  joinedAsOrgAdmin: boolean;
}

export {
  AUDIT_ACTIONS,
  type AuditAction,
  type AuditPerson,
  type AuditRecord,
} from '../logic/audit.ts';
import type { AuditRecord } from '../logic/audit.ts';
import type { OrgApiTokens } from './api-tokens.ts';
import type { OidcProvider, OidcSettings } from './oidc.ts';
import type { WorkspaceInfo } from './workspace-types.ts';

export interface OrganizationPatch {
  name?: string;
  publicLinks?: boolean;
  allowCopyOut?: boolean;
  allowApiTokens?: boolean;
}

/**
 * The organization's administration, and the audit log of a workspace. Every
 * call rejects with a `BackendError` as the backend's do; for anybody but an
 * administrator of the organization it is not there (404).
 */
export interface Organizations {
  oidcProvider(orgId: string): Promise<OidcProvider | undefined>;
  saveOidcProvider(orgId: string, settings: OidcSettings): Promise<OidcProvider>;
  deleteOidcProvider(orgId: string): Promise<void>;
  /** Throws the provider's pending settings away. */
  discardOidcPending(orgId: string): Promise<void>;
  /**
   * Ends, before its time, the keeping of the provider the organization moved
   * away from: the one read, kept since `createdAt`. 409 if another is kept by now.
   */
  endOidcFormer(orgId: string, createdAt: number): Promise<void>;
  /** Starts a sign-in through the pending settings: the address to send the browser to. */
  startOidcTrial(orgId: string): Promise<string>;
  /**
   * The organization the reader administers, of those of `workspaces` (the
   * reader's, as the shell read them); undefined when they administer none.
   */
  administered(workspaces: readonly WorkspaceInfo[]): Promise<string | undefined>;
  /**
   * Starts an organization of people with its first workspace, both under
   * `name`, which the reader administers; that workspace as their list has it
   * from then on. 400 with `too_many_organizations` and its `limit` past the
   * organizations an account may start; 403 with `invitation_required` on a
   * server that takes one organization.
   */
  createOrganization(name: string): Promise<WorkspaceInfo>;
  organization(orgId: string): Promise<Organization>;
  changeOrganization(orgId: string, patch: OrganizationPatch): Promise<Organization>;
  /** Its members, the oldest first. */
  members(orgId: string): Promise<OrgMember[]>;
  setRole(orgId: string, userId: string, role: OrgRole): Promise<void>;
  /** Removes a member from the organization. */
  removeMember(orgId: string, userId: string): Promise<void>;
  /** Every workspace, in use and in the trash, the oldest first. */
  workspaces(orgId: string): Promise<OrgWorkspace[]>;
  createWorkspace(orgId: string, name: string): Promise<OrgWorkspace>;
  renameWorkspace(orgId: string, workspaceId: string, name: string): Promise<OrgWorkspace>;
  deleteWorkspace(orgId: string, workspaceId: string): Promise<void>;
  restoreWorkspace(orgId: string, workspaceId: string): Promise<OrgWorkspace>;
  joinWorkspace(orgId: string, workspaceId: string): Promise<OrgWorkspace>;
  leaveWorkspace(orgId: string, workspaceId: string): Promise<OrgWorkspace>;
  /** What was done to the organization itself, the newest first. */
  records(orgId: string): Promise<AuditRecord[]>;
  /** The audit log of a workspace the reader works in, the newest first. */
  workspaceRecords(workspaceId: string): Promise<AuditRecord[]>;
  /** The personal API tokens of its members that reach its workspaces. */
  readonly apiTokens: OrgApiTokens;
}
