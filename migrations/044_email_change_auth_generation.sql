-- An email change cuts off the previous mailbox. Bump auth_generation in the
-- same UPDATE, like a password set (020) or a withdrawal (025): password
-- reset, login and email-change links already mailed, and pending MFA
-- challenges, then fail their generation check. Sessions do not carry the
-- generation and stay signed in.
--
-- 025 is pinned, so the function is replaced here with the same signature,
-- body, SECURITY DEFINER and search_path. CREATE OR REPLACE keeps its owner
-- and privileges (EXECUTE for the app role only; see grant-app-role.sql).
CREATE OR REPLACE FUNCTION fvoci.app_user_update_email(p_id uuid, p_email text)
RETURNS text
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    -- Same canonical form as users/magic_tokens: printable ASCII, lower case,
    -- one local part and one domain.
    IF p_email IS NULL
       OR p_email !~ '^[!-~]+$'
       OR p_email <> lower(p_email COLLATE "C")
       OR p_email !~ '^[^@]+@[^@]+$'
       OR length(p_email) > 320 THEN
        RAISE EXCEPTION 'app_user_update_email: invalid email';
    END IF;
    BEGIN
        UPDATE fvoci.users
        SET email = p_email,
            email_verified_at = now(),
            auth_generation = auth_generation + 1,
            updated_at = now()
        WHERE id = p_id
          AND deleted_at IS NULL;
        GET DIAGNOSTICS updated = ROW_COUNT;
    EXCEPTION WHEN unique_violation THEN
        RETURN 'email_taken';
    END;
    IF updated = 1 THEN
        RETURN 'ok';
    END IF;
    RETURN 'not_found';
END;
$$;
