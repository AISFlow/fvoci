-- Apply after migrations using a dedicated app role (not the migration owner).
-- Usage: psql "$DATABASE_URL" -v app_role=fvoci_app_xxx -f scripts/grant-app-role.sql

GRANT USAGE ON SCHEMA fvoci TO :"app_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA fvoci TO :"app_role";
GRANT USAGE ON SEQUENCE fvoci.events_seq TO :"app_role";

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
    email, given_name, family_name, text_scale, locale, timezone, week_starts_on,
    created_at, updated_at, deleted_at, email_verified_at, anonymized_at, suspended_at,
    is_instance_admin, auth_generation, personal_workspace_id
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
