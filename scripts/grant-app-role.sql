-- Apply after migrations using a dedicated app role (not the migration owner).
-- Usage: DATABASE_URL=<owner url> fvoci-migrate --grant-app-role fvoci_app_xxx
-- The command runs this file as one transaction: the broad table grant below is
-- only committed together with the narrowing revokes that follow it.
-- Manual psql must be equivalent: psql -X -v ON_ERROR_STOP=1 --single-transaction
--   -v app_role=fvoci_app_xxx -f scripts/grant-app-role.sql

GRANT USAGE ON SCHEMA fvoci TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA fvoci TO :"app_role";
GRANT USAGE ON SEQUENCE fvoci.events_seq TO :"app_role";

REVOKE ALL ON fvoci.schema_migrations FROM :"app_role";
GRANT SELECT ON fvoci.schema_migrations TO :"app_role";

REVOKE UPDATE, DELETE ON fvoci.events FROM :"app_role";
REVOKE UPDATE, DELETE ON fvoci.audit_log FROM :"app_role";
REVOKE DELETE ON fvoci.users FROM :"app_role";

REVOKE SELECT, UPDATE ON fvoci.users FROM :"app_role";
GRANT SELECT (
    id, email, given_name, family_name, text_scale, locale, timezone, week_starts_on,
    created_at, updated_at, deleted_at, email_verified_at, anonymized_at, suspended_at,
    is_instance_admin, auth_generation, personal_workspace_id
) ON fvoci.users TO :"app_role";
GRANT UPDATE (
    given_name, family_name, text_scale, locale, timezone, week_starts_on,
    personal_workspace_id, updated_at
) ON fvoci.users TO :"app_role";

REVOKE SELECT, UPDATE ON fvoci.sessions FROM :"app_role";
GRANT SELECT (
    id, user_id, expires_at, revoked_at, created_at, updated_at
) ON fvoci.sessions TO :"app_role";
GRANT UPDATE (
    expires_at, revoked_at, created_at, updated_at
) ON fvoci.sessions TO :"app_role";

REVOKE EXECUTE ON FUNCTION fvoci.app_user_password_hash(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_password_hash(uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_session_by_token_hash(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_session_by_token_hash(text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_rehash_password_hash(uuid, text, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_rehash_password_hash(uuid, text, text) TO :"app_role";

REVOKE EXECUTE ON FUNCTION public.app_tenant_id() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_tenant_id() TO :"app_role";
REVOKE EXECUTE ON FUNCTION public.app_system_ctx_on() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_system_ctx_on() TO :"app_role";
REVOKE EXECUTE ON FUNCTION public.app_self_user_id() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_self_user_id() TO :"app_role";
REVOKE EXECUTE ON FUNCTION public.app_invitation_token_hash() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_invitation_token_hash() TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_quota_billable_users(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_quota_billable_users(uuid) TO :"app_role";

GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.documents TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.document_states TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.document_collab_updates TO :"app_role";
GRANT SELECT, INSERT ON fvoci.document_collab_op_receipts TO :"app_role";
REVOKE UPDATE, DELETE ON fvoci.document_collab_op_receipts FROM :"app_role";

GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.attachments TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.attachment_text TO :"app_role";

GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.projects TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.project_members TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.groups TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.group_members TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.document_members TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.workflows TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.statuses TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.tasks TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.labels TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.task_assignees TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.task_labels TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.milestones TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.task_dependencies TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.invitations TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.revisions TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.comments TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.api_tokens TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.notifications TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.notification_prefs TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.workspace_holidays TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.ics_tokens TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.webhooks TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.webhook_deliveries TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.github_installations TO :"app_role";
GRANT SELECT, INSERT, DELETE ON fvoci.github_install_states TO :"app_role";
REVOKE UPDATE ON fvoci.github_install_states FROM :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.github_issue_links TO :"app_role";
GRANT SELECT, INSERT, DELETE ON fvoci.github_deliveries TO :"app_role";
REVOKE UPDATE ON fvoci.github_deliveries FROM :"app_role";
GRANT SELECT, INSERT ON fvoci.task_activity TO :"app_role";
REVOKE UPDATE, DELETE ON fvoci.task_activity FROM :"app_role";

GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.stars TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.document_tags TO :"app_role";
GRANT SELECT, INSERT, DELETE ON fvoci.document_tag_assignments TO :"app_role";
REVOKE UPDATE ON fvoci.document_tag_assignments FROM :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.collections TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.collection_items TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.collection_fields TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.collection_options TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.collection_values TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.collection_choices TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.collection_people TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.collection_views TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.views TO :"app_role";
-- share_links.token_hash is write-only for the app role: lookups go through
-- fvoci.app_share_link_by_token_hash, and rows are never updated in place.
GRANT INSERT, DELETE ON fvoci.share_links TO :"app_role";
REVOKE SELECT, UPDATE ON fvoci.share_links FROM :"app_role";
GRANT SELECT (
    id, workspace_id, user_id, document_id, project_id, expires_at, created_at, updated_at
) ON fvoci.share_links TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_share_link_by_token_hash(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_share_link_by_token_hash(text) TO :"app_role";

REVOKE ALL ON fvoci.magic_tokens FROM :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_set_password_hash(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_set_password_hash(uuid, text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_magic_issue(text, text, uuid, integer, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_magic_issue(text, text, uuid, integer, timestamptz) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_magic_consume(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_magic_consume(text) TO :"app_role";

REVOKE ALL ON fvoci.magic_tokens FROM :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_set_password_hash(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_set_password_hash(uuid, text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_magic_issue(text, text, uuid, integer, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_magic_issue(text, text, uuid, integer, timestamptz) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_magic_consume(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_magic_consume(text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_magic_purge_expired(timestamptz, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_magic_purge_expired(timestamptz, integer) TO :"app_role";

REVOKE EXECUTE ON FUNCTION fvoci.app_claim_attachment_extract() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_claim_attachment_extract() TO :"app_role";

REVOKE ALL ON fvoci.outbox_consumers FROM :"app_role";
REVOKE ALL ON fvoci.outbox_failures FROM :"app_role";
REVOKE ALL ON fvoci.processed_events FROM :"app_role";

REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_ensure_consumer(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_ensure_consumer(text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_lease(text, uuid, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_lease(text, uuid, integer) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_release(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_release(text, uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_read(text, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_read(text, integer) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_advance(text, uuid, xid8, bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_advance(text, uuid, xid8, bigint) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_record_failure(text, uuid, uuid, text, integer, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_record_failure(text, uuid, uuid, text, integer, integer) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_clear_failure(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_clear_failure(text, uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_failure_state(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_failure_state(text, uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_requeue(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_requeue(text, uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_claim_retries(text, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_claim_retries(text, integer) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_mark_processed(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_mark_processed(text, uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_is_processed(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_is_processed(text, uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_outbox_gc_processed(text, integer, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_outbox_gc_processed(text, integer, integer) TO :"app_role";

REVOKE EXECUTE ON FUNCTION fvoci.app_magic_issue_email_change(text, uuid, integer, text, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_magic_issue_email_change(text, uuid, integer, text, timestamptz) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_magic_consume_payload(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_magic_consume_payload(text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_withdraw(uuid, timestamptz, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_withdraw(uuid, timestamptz, text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_id_by_withdraw_cancel_token_hash(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_id_by_withdraw_cancel_token_hash(text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_restore_withdrawn(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_restore_withdrawn(uuid, text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_anonymize(uuid, text, text, timestamptz, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_anonymize(uuid, text, text, timestamptz, timestamptz) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_update_email(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_update_email(uuid, text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_user_mark_email_verified(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_user_mark_email_verified(uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_attachments_scrub_uploader(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_attachments_scrub_uploader(uuid, text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_attachments_stored_by_uploader(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_attachments_stored_by_uploader(uuid) TO :"app_role";
GRANT SELECT, INSERT ON fvoci.legal_documents TO :"app_role";
REVOKE UPDATE, DELETE ON fvoci.legal_documents FROM :"app_role";
GRANT SELECT, INSERT ON fvoci.user_consents TO :"app_role";
REVOKE UPDATE, DELETE ON fvoci.user_consents FROM :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.instance_settings TO :"app_role";
GRANT SELECT, UPDATE ON fvoci.instance_settings_meta TO :"app_role";
REVOKE INSERT, DELETE ON fvoci.instance_settings_meta FROM :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_session_consent_pending(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_session_consent_pending(text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_admin_set_instance_admin(uuid, boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_admin_set_instance_admin(uuid, boolean) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_admin_set_suspended(uuid, boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_admin_set_suspended(uuid, boolean) TO :"app_role";

-- 029 MFA / OIDC.
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.user_mfa TO :"app_role";
GRANT SELECT, INSERT, DELETE ON fvoci.identity_links TO :"app_role";
REVOKE UPDATE ON fvoci.identity_links FROM :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.workspace_oidc TO :"app_role";
REVOKE ALL ON fvoci.mfa_challenges FROM :"app_role";
REVOKE ALL ON fvoci.oidc_states FROM :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_mfa_challenge_issue(text, uuid, integer, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_mfa_challenge_issue(text, uuid, integer, timestamptz) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_mfa_challenge_peek(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_mfa_challenge_peek(text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_mfa_challenge_consume(text, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_mfa_challenge_consume(text, uuid) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_mfa_verify_attempt(uuid, integer, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_mfa_verify_attempt(uuid, integer, integer) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_oidc_state_issue(text, text, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_oidc_state_issue(text, text, timestamptz) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_oidc_state_consume(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_oidc_state_consume(text) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_auth_ephemeral_purge_expired(timestamptz, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_auth_ephemeral_purge_expired(timestamptz, integer) TO :"app_role";
REVOKE EXECUTE ON FUNCTION fvoci.app_workspace_sso_id_by_slug(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_workspace_sso_id_by_slug(text) TO :"app_role";
