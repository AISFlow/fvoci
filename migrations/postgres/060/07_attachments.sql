-- FVOCI PostgreSQL baseline fvoci-postgres-060, step 07: attachments.
-- Uploaded objects (hanging off exactly one document or one task), extracted
-- text chunks with embeddings, the storage-key cleanup journal and the worker
-- claim functions for text extraction and image previews.

CREATE TABLE fvoci.attachments (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    document_id uuid,
    uploader_id uuid NOT NULL REFERENCES fvoci.users (id),
    status text NOT NULL,
    name text NOT NULL,
    mime text NOT NULL DEFAULT 'application/octet-stream',
    declared_mime text,
    size_bytes bigint,
    reserved_size_bytes bigint NOT NULL,
    storage_key text NOT NULL,
    image boolean NOT NULL DEFAULT false,
    variants jsonb NOT NULL DEFAULT '{}'::jsonb,
    extract_text text NOT NULL DEFAULT '',
    extract_status text NOT NULL DEFAULT 'skipped',
    scan_status text NOT NULL DEFAULT 'skipped',
    upload_meta jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    extract_lease_token uuid,
    extract_lease_expires_at timestamptz,
    extract_attempts smallint NOT NULL DEFAULT 0,
    extract_warnings jsonb NOT NULL DEFAULT '[]'::jsonb,
    extract_rhwp_rev text,
    task_id uuid,
    preview_status text NOT NULL DEFAULT 'skipped',
    preview_lease_token uuid,
    preview_lease_expires_at timestamptz,
    preview_attempts smallint NOT NULL DEFAULT 0,
    CONSTRAINT attachments_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT attachments_storage_key_unique UNIQUE (storage_key),
    CONSTRAINT attachments_status_check CHECK (status IN ('uploading', 'assembling', 'stored')),
    CONSTRAINT attachments_scan_status_check CHECK (scan_status IN ('skipped', 'clean', 'infected')),
    CONSTRAINT attachments_extract_status_check CHECK (
        extract_status IN (
            'pending', 'skipped', 'ok', 'empty', 'partial', 'unsupported',
            'corrupt', 'resource_limit', 'worker_failure'
        )
    ),
    CONSTRAINT attachments_reserved_size_check CHECK (
        reserved_size_bytes > 0 AND reserved_size_bytes <= 9007199254740991
    ),
    CONSTRAINT attachments_stored_size_check CHECK (
        (
            status = 'stored'
            AND size_bytes IS NOT NULL
            AND size_bytes = reserved_size_bytes
            AND completed_at IS NOT NULL
        )
        OR (
            status <> 'stored'
            AND size_bytes IS NULL
            AND completed_at IS NULL
        )
    ),
    CONSTRAINT attachments_workspace_document_fk
        FOREIGN KEY (workspace_id, document_id)
        REFERENCES fvoci.documents (workspace_id, id),
    CONSTRAINT attachments_extract_lease_check
        CHECK ((extract_lease_token IS NULL) = (extract_lease_expires_at IS NULL)),
    CONSTRAINT attachments_extract_attempts_check
        CHECK (extract_attempts >= 0 AND extract_attempts <= 2),
    CONSTRAINT attachments_extract_warnings_check
        CHECK (jsonb_typeof(extract_warnings) = 'array' AND jsonb_array_length(extract_warnings) <= 32),
    CONSTRAINT attachments_parent_xor_check
        CHECK ((document_id IS NULL) <> (task_id IS NULL)),
    CONSTRAINT attachments_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id),
    CONSTRAINT attachments_preview_status_check
        CHECK (preview_status IN ('pending', 'skipped', 'ok', 'failed')),
    CONSTRAINT attachments_preview_lease_check
        CHECK ((preview_lease_token IS NULL) = (preview_lease_expires_at IS NULL)),
    CONSTRAINT attachments_preview_attempts_check
        CHECK (preview_attempts >= 0 AND preview_attempts <= 3)
);

CREATE INDEX attachments_workspace_id_document_id_idx
    ON fvoci.attachments (workspace_id, document_id);
CREATE INDEX attachments_uploader_id_idx ON fvoci.attachments (uploader_id);
CREATE INDEX attachments_uploading_created_at_idx
    ON fvoci.attachments (status, created_at)
    WHERE status IN ('uploading', 'assembling');
CREATE INDEX attachments_uploader_id_stored_idx
    ON fvoci.attachments (uploader_id, created_at, id)
    WHERE status = 'stored';
CREATE INDEX attachments_extract_pending_idx
    ON fvoci.attachments (completed_at, id)
    WHERE status = 'stored' AND extract_status = 'pending';
CREATE INDEX attachments_workspace_id_task_id_idx
    ON fvoci.attachments (workspace_id, task_id);
CREATE INDEX attachments_preview_pending_idx
    ON fvoci.attachments (completed_at, id)
    WHERE status = 'stored' AND preview_status = 'pending';

-- attachments is not FORCE RLS: the migration owner and definers see every row.
ALTER TABLE fvoci.attachments ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.attachments
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.attachment_text (
    workspace_id uuid NOT NULL,
    attachment_id uuid NOT NULL,
    chunk_no integer NOT NULL,
    start_offset integer NOT NULL,
    end_offset integer NOT NULL,
    text text NOT NULL,
    chosung text NOT NULL DEFAULT '',
    status text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    -- One vector per extracted chunk (jsonb, 1536 floats), written by the
    -- embedding pass and copied into Meili `_vectors.attachments`.
    embedding jsonb,
    PRIMARY KEY (workspace_id, attachment_id, chunk_no),
    CONSTRAINT attachment_text_chunk_no_check CHECK (chunk_no >= 0),
    CONSTRAINT attachment_text_offsets_check CHECK (
        start_offset >= 0 AND end_offset >= start_offset
    ),
    CONSTRAINT attachment_text_status_check CHECK (status IN (
        'ok', 'skipped', 'empty', 'partial', 'unsupported', 'corrupt',
        'resource_limit', 'worker_failure'
    )),
    CONSTRAINT attachment_text_parent_fk
        FOREIGN KEY (workspace_id, attachment_id)
        REFERENCES fvoci.attachments (workspace_id, id)
        ON DELETE CASCADE,
    CONSTRAINT attachment_text_embedding_shape_check CHECK (
        embedding IS NULL
        OR (jsonb_typeof(embedding) = 'array' AND jsonb_array_length(embedding) = 1536)
    )
);

CREATE INDEX attachment_text_workspace_attachment_idx
    ON fvoci.attachment_text (workspace_id, attachment_id);
-- Pending-embedding lookup per workspace.
CREATE INDEX attachment_text_pending_embedding_idx
    ON fvoci.attachment_text (workspace_id, attachment_id)
    WHERE embedding IS NULL AND text <> '';

ALTER TABLE fvoci.attachment_text ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.attachment_text FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.attachment_text
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()))
    WITH CHECK (workspace_id = (SELECT public.app_tenant_id()));

-- Storage keys outlive attachment and workspace deletion until the object is
-- confirmed gone. No FK, so a workspace cascade cannot erase a reclaim identity.
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

-- Every deleted attachment row journals its original key (and its preview key
-- when one was published) in the same transaction.
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

-- Text extraction claims: a live task parent qualifies as well as a live
-- document. Exhausted leases become worker_failure; orphaned parents skip.
CREATE FUNCTION fvoci.app_claim_attachment_extract()
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

-- Image previews. A completed attachment whose sniffed MIME is in the preview
-- codec set starts `pending`; the preview job leases it here, journals the new
-- object key in attachment_object_cleanups before writing it, and publishes
-- variants.preview while deleting that journal row in one transaction.
CREATE FUNCTION fvoci.app_claim_attachment_preview(p_lease_secs integer)
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
    IF p_lease_secs IS NULL OR p_lease_secs < 1 OR p_lease_secs > 3600 THEN
        RAISE EXCEPTION 'app_claim_attachment_preview: lease must be 1..3600 seconds';
    END IF;

    -- Leases that expired three times: give up (source job attempts).
    UPDATE fvoci.attachments AS a
    SET preview_status = 'failed',
        preview_lease_token = NULL,
        preview_lease_expires_at = NULL
    FROM (
        SELECT a2.workspace_id, a2.id
        FROM fvoci.attachments AS a2
        WHERE a2.status = 'stored'
          AND a2.preview_status = 'pending'
          AND a2.preview_attempts >= 3
          AND a2.preview_lease_expires_at IS NOT NULL
          AND a2.preview_lease_expires_at < pg_catalog.now()
        ORDER BY a2.completed_at, a2.id
        LIMIT 50
        FOR UPDATE OF a2 SKIP LOCKED
    ) AS exhausted
    WHERE a.workspace_id = exhausted.workspace_id
      AND a.id = exhausted.id;

    SELECT a.workspace_id, a.id, a.preview_attempts
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
      AND a.preview_status = 'pending'
      AND a.preview_attempts < 3
      AND (
          a.preview_lease_expires_at IS NULL
          OR a.preview_lease_expires_at < pg_catalog.now()
      )
    ORDER BY a.completed_at, a.id
    LIMIT 1
    FOR UPDATE OF a SKIP LOCKED;

    IF NOT FOUND THEN
        RETURN;
    END IF;

    new_token := gen_random_uuid();
    UPDATE fvoci.attachments AS a
    SET preview_lease_token = new_token,
        preview_lease_expires_at = pg_catalog.now() + (p_lease_secs * interval '1 second'),
        preview_attempts = a.preview_attempts + 1
    WHERE a.workspace_id = picked_workspace_id
      AND a.id = picked_attachment_id;

    workspace_id := picked_workspace_id;
    attachment_id := picked_attachment_id;
    lease_token := new_token;
    attempt := picked_attempts + 1;
    RETURN NEXT;
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

REVOKE ALL ON FUNCTION fvoci.app_claim_attachment_extract() FROM PUBLIC;
REVOKE ALL ON FUNCTION fvoci.app_claim_attachment_preview(integer) FROM PUBLIC;
