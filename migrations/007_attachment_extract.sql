ALTER TABLE fvoci.attachments
    ADD COLUMN extract_lease_token uuid,
    ADD COLUMN extract_lease_expires_at timestamptz,
    ADD COLUMN extract_attempts smallint NOT NULL DEFAULT 0,
    ADD COLUMN extract_warnings jsonb NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN extract_rhwp_rev text;

ALTER TABLE fvoci.attachments
    ADD CONSTRAINT attachments_extract_lease_check
        CHECK ((extract_lease_token IS NULL) = (extract_lease_expires_at IS NULL)),
    ADD CONSTRAINT attachments_extract_attempts_check
        CHECK (extract_attempts >= 0 AND extract_attempts <= 2),
    ADD CONSTRAINT attachments_extract_warnings_check
        CHECK (jsonb_typeof(extract_warnings) = 'array' AND jsonb_array_length(extract_warnings) <= 32);

CREATE INDEX attachments_extract_pending_idx
    ON fvoci.attachments (completed_at, id)
    WHERE status = 'stored' AND extract_status = 'pending';

CREATE OR REPLACE FUNCTION fvoci.app_claim_attachment_extract()
RETURNS TABLE (
    workspace_id uuid,
    attachment_id uuid,
    lease_token uuid,
    attempt smallint
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    picked_workspace_id uuid;
    picked_attachment_id uuid;
    picked_attempts smallint;
    new_token uuid;
BEGIN
    UPDATE fvoci.attachments AS a
    SET
        extract_status = 'worker_failure',
        extract_text = '',
        extract_warnings = '[]'::jsonb,
        extract_rhwp_rev = NULL,
        extract_lease_token = NULL,
        extract_lease_expires_at = NULL
    FROM (
        SELECT a2.workspace_id, a2.id
        FROM fvoci.attachments AS a2
        WHERE a2.status = 'stored'
          AND a2.extract_status = 'pending'
          AND a2.extract_attempts >= 2
          AND a2.extract_lease_expires_at IS NOT NULL
          AND a2.extract_lease_expires_at < pg_catalog.now()
        ORDER BY a2.completed_at, a2.id
        LIMIT 50
        FOR UPDATE OF a2 SKIP LOCKED
    ) AS exhausted
    WHERE a.workspace_id = exhausted.workspace_id
      AND a.id = exhausted.id
      AND a.status = 'stored'
      AND a.extract_status = 'pending'
      AND a.extract_attempts >= 2
      AND a.extract_lease_expires_at IS NOT NULL
      AND a.extract_lease_expires_at < pg_catalog.now();

    UPDATE fvoci.attachments AS a
    SET
        extract_status = 'skipped',
        extract_text = '',
        extract_warnings = '[]'::jsonb,
        extract_rhwp_rev = NULL,
        extract_lease_token = NULL,
        extract_lease_expires_at = NULL
    FROM (
        SELECT a2.workspace_id, a2.id
        FROM fvoci.attachments AS a2
        LEFT JOIN fvoci.documents AS d
            ON d.workspace_id = a2.workspace_id
           AND d.id = a2.document_id
           AND d.deleted_at IS NULL
        LEFT JOIN fvoci.workspaces AS w
            ON w.id = a2.workspace_id
           AND w.deleted_at IS NULL
        WHERE a2.status = 'stored'
          AND a2.extract_status = 'pending'
          AND (d.id IS NULL OR w.id IS NULL)
          AND (
              a2.extract_lease_expires_at IS NULL
              OR a2.extract_lease_expires_at < pg_catalog.now()
          )
        ORDER BY a2.completed_at, a2.id
        LIMIT 50
        FOR UPDATE OF a2 SKIP LOCKED
    ) AS orphaned
    WHERE a.workspace_id = orphaned.workspace_id
      AND a.id = orphaned.id
      AND a.status = 'stored'
      AND a.extract_status = 'pending'
      AND (
          a.extract_lease_expires_at IS NULL
          OR a.extract_lease_expires_at < pg_catalog.now()
      );

    SELECT
        a.workspace_id,
        a.id,
        a.extract_attempts
    INTO picked_workspace_id, picked_attachment_id, picked_attempts
    FROM fvoci.attachments AS a
    INNER JOIN fvoci.documents AS d
        ON d.workspace_id = a.workspace_id
       AND d.id = a.document_id
       AND d.deleted_at IS NULL
    INNER JOIN fvoci.workspaces AS w
        ON w.id = a.workspace_id
       AND w.deleted_at IS NULL
    WHERE a.status = 'stored'
      AND a.extract_status = 'pending'
      AND a.extract_attempts < 2
      AND (
          a.extract_lease_expires_at IS NULL
          OR a.extract_lease_expires_at < pg_catalog.now()
      )
    ORDER BY a.completed_at, a.id
    LIMIT 1
    FOR UPDATE OF a SKIP LOCKED;

    IF NOT FOUND THEN
        RETURN;
    END IF;

    new_token := gen_random_uuid();

    UPDATE fvoci.attachments AS a
    SET
        extract_lease_token = new_token,
        extract_lease_expires_at = pg_catalog.now() + interval '300 seconds',
        extract_attempts = a.extract_attempts + 1
    WHERE a.workspace_id = picked_workspace_id
      AND a.id = picked_attachment_id;

    workspace_id := picked_workspace_id;
    attachment_id := picked_attachment_id;
    lease_token := new_token;
    attempt := picked_attempts + 1;
    RETURN NEXT;
END;
$$;

REVOKE ALL ON FUNCTION fvoci.app_claim_attachment_extract() FROM PUBLIC;
