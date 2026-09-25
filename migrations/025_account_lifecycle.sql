-- Account lifecycle: withdraw grace period, withdraw cancel token, final
-- anonymization, email change and magic-link login tokens.
--
-- Numbered 025 because 023/024 are reserved by open branches; the coordinator
-- renumbers at integration.
--
-- The app role keeps its column-level grants on fvoci.users: it cannot read
-- withdraw_cancel_token_hash and cannot write email, deleted_at,
-- anonymized_at, email_verified_at or auth_generation. Every such write goes
-- through a narrow SECURITY DEFINER function below.

ALTER TABLE fvoci.users ADD COLUMN withdraw_cancel_token_hash text;

CREATE UNIQUE INDEX users_withdraw_cancel_token_hash_unique
    ON fvoci.users (withdraw_cancel_token_hash)
    WHERE withdraw_cancel_token_hash IS NOT NULL;

CREATE INDEX users_withdrawn_due_idx
    ON fvoci.users (deleted_at)
    WHERE deleted_at IS NOT NULL AND anonymized_at IS NULL;

-- Source keeps login / password_reset / email_change payloads in one Redis
-- namespace (magic:<hash>) and GETDELs before checking the kind.
ALTER TABLE fvoci.magic_tokens DROP CONSTRAINT magic_tokens_kind_check;
ALTER TABLE fvoci.magic_tokens
    ADD CONSTRAINT magic_tokens_kind_check
    CHECK (kind IN ('password_reset', 'login', 'email_change'));
ALTER TABLE fvoci.magic_tokens ADD COLUMN new_email text;
ALTER TABLE fvoci.magic_tokens
    ADD CONSTRAINT magic_tokens_new_email_check CHECK (
        (kind = 'email_change') = (new_email IS NOT NULL)
        AND (new_email IS NULL OR (new_email ~ '^[!-~]+$' AND new_email = lower(new_email COLLATE "C")))
    );

CREATE FUNCTION fvoci.app_magic_issue_email_change(
    p_hash text,
    p_user_id uuid,
    p_generation integer,
    p_new_email text,
    p_expires_at timestamptz
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    INSERT INTO fvoci.magic_tokens (token_hash, kind, user_id, generation, expires_at, new_email)
    VALUES (p_hash, 'email_change', p_user_id, p_generation, p_expires_at, p_new_email);
END;
$$;

-- GETDEL with the email-change payload. Expired rows yield nothing.
CREATE FUNCTION fvoci.app_magic_consume_payload(p_hash text)
RETURNS TABLE (kind text, user_id uuid, generation integer, new_email text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    RETURN QUERY
    DELETE FROM fvoci.magic_tokens
    WHERE token_hash = p_hash
      AND expires_at > now()
    RETURNING fvoci.magic_tokens.kind, fvoci.magic_tokens.user_id,
              fvoci.magic_tokens.generation, fvoci.magic_tokens.new_email;
END;
$$;

-- Source markWithdrawn + setWithdrawCancelTokenHash in one statement.
-- auth_generation bumps so outstanding magic tokens fail their check.
CREATE FUNCTION fvoci.app_user_withdraw(p_id uuid, p_at timestamptz, p_cancel_hash text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF p_at IS NULL OR p_cancel_hash IS NULL OR p_cancel_hash !~ '^[0-9a-f]{64}$' THEN
        RAISE EXCEPTION 'app_user_withdraw: invalid arguments';
    END IF;
    UPDATE fvoci.users
    SET deleted_at = p_at,
        withdraw_cancel_token_hash = p_cancel_hash,
        auth_generation = auth_generation + 1,
        updated_at = now()
    WHERE id = p_id
      AND deleted_at IS NULL
      AND anonymized_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

CREATE FUNCTION fvoci.app_user_id_by_withdraw_cancel_token_hash(p_hash text)
RETURNS uuid
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT id FROM fvoci.users
    WHERE withdraw_cancel_token_hash = p_hash
      AND deleted_at IS NOT NULL
      AND anonymized_at IS NULL
$$;

-- Source restoreWithdrawn: clear deleted_at and the one-time cancel token.
-- The definer itself requires the matching cancel hash and an unexpired grace
-- period (source deadline: now < deleted_at + 14 days).
CREATE FUNCTION fvoci.app_user_restore_withdrawn(p_id uuid, p_cancel_hash text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF p_cancel_hash IS NULL OR p_cancel_hash !~ '^[0-9a-f]{64}$' THEN
        RETURN false;
    END IF;
    UPDATE fvoci.users
    SET deleted_at = NULL,
        withdraw_cancel_token_hash = NULL,
        updated_at = now()
    WHERE id = p_id
      AND withdraw_cancel_token_hash = p_cancel_hash
      AND deleted_at IS NOT NULL
      AND deleted_at > now() - interval '14 days'
      AND anonymized_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

-- Source anonymize: only a row whose 14-day grace period has ended. The
-- cutoff is capped inside the definer: a later p_due_before can never erase
-- early. Clears name, email, password hash and the cancel token in one UPDATE.
CREATE FUNCTION fvoci.app_user_anonymize(
    p_id uuid,
    p_given_name text,
    p_email text,
    p_at timestamptz,
    p_due_before timestamptz
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF p_email IS NULL OR p_email !~ '^withdrawn-[0-9a-f]{12}@withdrawn\.invalid$' THEN
        RAISE EXCEPTION 'app_user_anonymize: invalid placeholder email';
    END IF;
    IF p_at IS NULL OR p_due_before IS NULL THEN
        RAISE EXCEPTION 'app_user_anonymize: time arguments are required';
    END IF;
    UPDATE fvoci.users
    SET given_name = p_given_name,
        family_name = NULL,
        email = p_email,
        password_hash = NULL,
        withdraw_cancel_token_hash = NULL,
        anonymized_at = p_at,
        auth_generation = auth_generation + 1,
        updated_at = now()
    WHERE id = p_id
      AND deleted_at IS NOT NULL
      AND deleted_at <= LEAST(p_due_before, now() - interval '14 days')
      AND anonymized_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

-- Source updateEmail: live row only; a unique violation is reported, not raised.
CREATE FUNCTION fvoci.app_user_update_email(p_id uuid, p_email text)
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

-- Source setEmailVerified for a consumed login link.
CREATE FUNCTION fvoci.app_user_mark_email_verified(p_id uuid)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    UPDATE fvoci.users
    SET email_verified_at = now(), updated_at = now()
    WHERE id = p_id AND deleted_at IS NULL;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated = 1;
END;
$$;

-- Source scrubNamesByUploader runs in a system transaction across every
-- workspace. Only an already anonymized uploader qualifies.
CREATE FUNCTION fvoci.app_attachments_scrub_uploader(p_uploader uuid, p_name text)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    updated integer;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM fvoci.users WHERE id = p_uploader AND anonymized_at IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'app_attachments_scrub_uploader: uploader is not anonymized';
    END IF;
    UPDATE fvoci.attachments SET name = p_name WHERE uploader_id = p_uploader;
    GET DIAGNOSTICS updated = ROW_COUNT;
    RETURN updated;
END;
$$;

-- Source listStoredByUploader (user export): the uploader's own stored
-- attachments across workspaces. Only a live (not withdrawn) user.
CREATE FUNCTION fvoci.app_attachments_stored_by_uploader(p_uploader uuid)
RETURNS TABLE (
    id uuid,
    workspace_id uuid,
    name text,
    mime text,
    size_bytes bigint,
    scan_status text,
    storage_key text
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
    SELECT a.id, a.workspace_id, a.name, a.mime, a.size_bytes, a.scan_status, a.storage_key
    FROM fvoci.attachments a
    INNER JOIN fvoci.users u ON u.id = a.uploader_id AND u.deleted_at IS NULL
    WHERE a.uploader_id = p_uploader
      AND a.status = 'stored'
    ORDER BY a.created_at ASC, a.id ASC
$$;
