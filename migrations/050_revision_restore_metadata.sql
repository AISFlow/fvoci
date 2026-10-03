-- Restore appends to the existing history; historical rows remain unchanged.
-- Source IDs remain provenance even if an independent retention policy later
-- removes an automatic source revision, so there is deliberately no cascade FK.
ALTER TABLE fvoci.revisions
    ADD COLUMN restored_from_id uuid,
    ADD COLUMN restore_correlation_id uuid,
    ADD COLUMN restore_base_tail_seq bigint,
    ADD COLUMN restore_committed_tail_seq bigint;

ALTER TABLE fvoci.revisions DROP CONSTRAINT revisions_reason_check;
ALTER TABLE fvoci.revisions
    ADD CONSTRAINT revisions_reason_check
        CHECK (reason IN ('manual', 'session', 'scheduled', 'restore')),
    ADD CONSTRAINT revisions_restore_metadata_check CHECK (
        (reason = 'restore'
            AND created_by IS NOT NULL
            AND restored_from_id IS NOT NULL
            AND restore_correlation_id IS NOT NULL
            AND restore_base_tail_seq IS NOT NULL
            AND restore_base_tail_seq >= 0
            AND restore_committed_tail_seq IS NOT NULL
            AND restore_committed_tail_seq > restore_base_tail_seq
            AND restore_committed_tail_seq - restore_base_tail_seq = 1)
        OR
        (reason <> 'restore'
            AND restored_from_id IS NULL
            AND restore_correlation_id IS NULL
            AND restore_base_tail_seq IS NULL
            AND restore_committed_tail_seq IS NULL)
    );

-- A correlation is a single workspace operation. Reuse for a different
-- actor/target/source/base must conflict, including after response loss.
CREATE UNIQUE INDEX revisions_workspace_restore_correlation_idx
    ON fvoci.revisions (workspace_id, restore_correlation_id)
    WHERE restore_correlation_id IS NOT NULL;
-- Existing tenant FORCE-RLS policy continues to protect all new columns.
