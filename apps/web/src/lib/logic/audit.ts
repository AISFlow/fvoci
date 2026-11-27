/**
 * The records of an audit log as the server answers them (`AuditRecord` of the
 * contract): who did what, to what and whom, and when.
 */
export const AUDIT_ACTIONS = [
  'self_added_to_container',
  'added_to_container',
  'org_admin_joined',
  'org_admin_left',
  'org_admin_revoked',
  'org_admin_granted',
  'org_admin_removed',
  'org_member_removed',
  'workspace_created',
  'workspace_renamed',
  'workspace_deleted',
  'workspace_restored',
  'public_links_disabled',
  'public_links_enabled',
  'oidc_provider_configured',
  'oidc_provider_removed',
  'sso_required',
  'sso_released',
  'oidc_provider_pending_saved',
  'oidc_provider_pending_applied',
  'oidc_provider_pending_discarded',
  'password_reset_link_made',
  'password_set_by_operator',
  'oidc_former_provider_kept',
  'oidc_former_provider_ended',
  'oidc_former_provider_lapsed',
  'oidc_identity_bound',
  'oidc_identity_carried_over',
  'email_verified_by_operator',
  'container_copied_out',
  'copy_out_disabled',
  'copy_out_enabled',
  'api_tokens_enabled',
  'api_tokens_disabled',
  'api_token_revoked',
] as const;

export type AuditAction = (typeof AUDIT_ACTIONS)[number];

export interface AuditPerson {
  id: string;
  /** Empty when the account is gone. */
  username: string;
  displayName: string;
}

/**
 * Who did it: a person by their account, the server's operator, who has none, or the
 * server itself, for what ends when its time runs out.
 */
export type AuditActor =
  (AuditPerson & { kind: 'person' }) | { kind: 'server_operator' } | { kind: 'server' };

/** One record: who did what, to what and whom, and when. */
export interface AuditRecord {
  id: string;
  action: AuditAction;
  actor: AuditActor;
  subject: AuditPerson | null;
  targetId: string;
  /** The board's title where the reader sees the board, the workspace's name; null otherwise. */
  targetName: string | null;
  createdAt: number;
  /**
   * The issuer a record of a provider's move names, the characters a token taken out
   * is shown by; null for the others.
   */
  detail: string | null;
  /**
   * Who the identity a record of `oidc_identity_bound` or `oidc_identity_carried_over`
   * tied is: the address its provider gave, or its subject. Null for the others, and in
   * a workspace's records.
   */
  identity: string | null;
}
