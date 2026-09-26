-- Admin user erasure cancel (source packages/core/src/consent.ts
-- cancelUserErasure({ actorAdminId, userId })).
--
-- Numbered 032 because 031 is reserved by an open branch.
--
-- The user's own cancel path (app_user_restore_withdrawn) requires the
-- one-time cancel hash, which an instance admin never sees. This definer is
-- the admin path: it does not weaken that function. The caller must have set
-- app.self_user_id to a live (not withdrawn, not suspended) instance admin,
-- and the 14-day grace period is checked here against the wall clock (the
-- caller already holds the target's row lock, so transaction-start now()
-- could be earlier than the moment the lock was granted).

CREATE FUNCTION fvoci.app_admin_user_restore_withdrawn(p_id uuid)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users a
        WHERE a.id = public.app_self_user_id()
          AND a.is_instance_admin
          AND a.deleted_at IS NULL
          AND a.suspended_at IS NULL
    ) THEN
        RAISE EXCEPTION 'instance admin required' USING ERRCODE = '42501';
    END IF;
    UPDATE fvoci.users
    SET deleted_at = NULL,
        withdraw_cancel_token_hash = NULL,
        updated_at = now()
    WHERE id = p_id
      AND deleted_at IS NOT NULL
      AND deleted_at > clock_timestamp() - interval '14 days'
      AND anonymized_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

REVOKE EXECUTE ON FUNCTION fvoci.app_admin_user_restore_withdrawn(uuid) FROM PUBLIC;
