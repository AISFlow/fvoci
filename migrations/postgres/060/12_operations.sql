-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 12: operator maintenance.
-- Secret maintenance (`fvoci-migrate --secrets-audit|--secrets-rotate`): the
-- operator commands run as the app role in the system context and see only
-- counts and a compare-and-set result through these definers, never password
-- hashes. Outbox lag and oldest-write-transaction probes are step 04 objects.

-- Password hashes grouped by pepper key id, with how many do not match the
-- current format. Same population as the source: every user row with a
-- password hash, deleted ones included (their hash still needs its key).
CREATE FUNCTION fvoci.app_password_key_inventory()
RETURNS TABLE(key_id text, total bigint, invalid bigint)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    IF (SELECT public.app_system_ctx_on()) IS NOT TRUE THEN
        RAISE EXCEPTION 'password inventory requires system context';
    END IF;
    RETURN QUERY
        SELECT
            CASE
                WHEN u.password_hash ~ '^\$fvoci-pepper=[a-zA-Z0-9_-]{1,32}\$'
                    THEN substring(u.password_hash FROM '^\$fvoci-pepper=([a-zA-Z0-9_-]{1,32})\$')
                ELSE 'unknown'
            END,
            count(*),
            count(*) FILTER (
                WHERE u.password_hash !~ '^\$fvoci-pepper=[a-zA-Z0-9_-]{1,32}\$argon2id\$v=19\$m=65536,t=3,p=1\$[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]\$[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]$'
            )
        FROM fvoci.users AS u
        WHERE u.password_hash IS NOT NULL
        GROUP BY 1;
END;
$$;

-- Re-seal the VAPID private key only while it still holds `p_expected` (the
-- source locks instance_config FOR UPDATE, which the app role cannot). A
-- concurrent `--rotate-vapid` that stored a new pair first makes this a no-op
-- and the caller reports a conflict instead of restoring the old key.
CREATE FUNCTION fvoci.app_replace_vapid_private(p_expected text, p_private text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    replaced boolean;
BEGIN
    IF (SELECT public.app_system_ctx_on()) IS NOT TRUE THEN
        RAISE EXCEPTION 'replace vapid requires system context';
    END IF;
    UPDATE fvoci.instance_config
    SET vapid_private_key = p_private
    WHERE id = 1 AND vapid_private_key = p_expected
    RETURNING true INTO replaced;
    RETURN coalesce(replaced, false);
END;
$$;

REVOKE ALL ON FUNCTION fvoci.app_password_key_inventory() FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_replace_vapid_private(text, text) FROM PUBLIC;
