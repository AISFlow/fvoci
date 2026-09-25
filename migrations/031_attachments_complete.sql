-- Source attachments: a row hangs off exactly one document or one task
-- (`attachments_parent_xor_check`, `attachments_workspace_task_fk`).
ALTER TABLE fvoci.attachments
    ALTER COLUMN document_id DROP NOT NULL,
    ADD COLUMN task_id uuid;

ALTER TABLE fvoci.attachments
    ADD CONSTRAINT attachments_parent_xor_check
        CHECK ((document_id IS NULL) <> (task_id IS NULL)),
    ADD CONSTRAINT attachments_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id);

CREATE INDEX attachments_workspace_id_task_id_idx
    ON fvoci.attachments (workspace_id, task_id);

-- Source attachment_object_cleanups: storage keys outlive attachment and
-- workspace deletion until the object is confirmed gone. No FK, so a
-- workspace cascade cannot erase a reclaim identity.
CREATE TABLE fvoci.attachment_object_cleanups (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL,
    attachment_id uuid NOT NULL,
    storage_key text NOT NULL,
    due_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    attempts integer NOT NULL DEFAULT 0,
    CONSTRAINT attachment_object_cleanups_attempts_check CHECK (attempts >= 0)
);

CREATE INDEX attachment_object_cleanups_due_idx
    ON fvoci.attachment_object_cleanups (due_at, id);

ALTER TABLE fvoci.attachment_object_cleanups ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.attachment_object_cleanups FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.attachment_object_cleanups
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Source attachment_object_cleanups_after_attachment_delete: every deleted
-- attachment row journals its original key (and its preview key when one was
-- published) in the same transaction.
CREATE FUNCTION fvoci.attachment_object_cleanups_after_attachment_delete()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    INSERT INTO fvoci.attachment_object_cleanups (workspace_id, attachment_id, storage_key)
    VALUES (OLD.workspace_id, OLD.id, OLD.storage_key);
    IF jsonb_typeof(OLD.variants -> 'preview' -> 'key') = 'string' THEN
        INSERT INTO fvoci.attachment_object_cleanups (workspace_id, attachment_id, storage_key)
        VALUES (OLD.workspace_id, OLD.id, OLD.variants -> 'preview' ->> 'key');
    END IF;
    RETURN OLD;
END;
$$;

CREATE TRIGGER attachment_object_cleanups_after_attachment_delete
    AFTER DELETE ON fvoci.attachments
    FOR EACH ROW EXECUTE FUNCTION fvoci.attachment_object_cleanups_after_attachment_delete();

-- Extract claims accept a live task parent as well as a live document
-- (source queues extract-text for every completed attachment).
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
        LEFT JOIN fvoci.tasks AS t
            ON t.workspace_id = a2.workspace_id
           AND t.id = a2.task_id
           AND t.deleted_at IS NULL
        LEFT JOIN fvoci.workspaces AS w
            ON w.id = a2.workspace_id
           AND w.deleted_at IS NULL
        WHERE a2.status = 'stored'
          AND a2.extract_status = 'pending'
          AND ((d.id IS NULL AND t.id IS NULL) OR w.id IS NULL)
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
    LEFT JOIN fvoci.documents AS d
        ON d.workspace_id = a.workspace_id
       AND d.id = a.document_id
       AND d.deleted_at IS NULL
    LEFT JOIN fvoci.tasks AS t
        ON t.workspace_id = a.workspace_id
       AND t.id = a.task_id
       AND t.deleted_at IS NULL
    INNER JOIN fvoci.workspaces AS w
        ON w.id = a.workspace_id
       AND w.deleted_at IS NULL
    WHERE (d.id IS NOT NULL OR t.id IS NOT NULL)
      AND a.status = 'stored'
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
