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

GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.documents TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.document_states TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.document_collab_updates TO :"app_role";
GRANT SELECT, INSERT ON fvoci.document_collab_op_receipts TO :"app_role";
REVOKE UPDATE, DELETE ON fvoci.document_collab_op_receipts FROM :"app_role";

GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.attachments TO :"app_role";

GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.projects TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.project_members TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.workflows TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.statuses TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON fvoci.tasks TO :"app_role";

REVOKE EXECUTE ON FUNCTION fvoci.app_claim_attachment_extract() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION fvoci.app_claim_attachment_extract() TO :"app_role";
