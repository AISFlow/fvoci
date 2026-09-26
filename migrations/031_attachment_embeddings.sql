-- Source `attachment_text.embedding` (jsonb, 1536 floats): one vector per extracted
-- chunk, written by the embedding pass and copied into Meili `_vectors.attachments`.
ALTER TABLE fvoci.attachment_text ADD COLUMN embedding jsonb;

ALTER TABLE fvoci.attachment_text ADD CONSTRAINT attachment_text_embedding_shape_check CHECK (
    embedding IS NULL
    OR (jsonb_typeof(embedding) = 'array' AND jsonb_array_length(embedding) = 1536)
);

-- Pending-embedding lookup per workspace.
CREATE INDEX attachment_text_pending_embedding_idx
    ON fvoci.attachment_text (workspace_id, attachment_id)
    WHERE embedding IS NULL AND text <> '';
