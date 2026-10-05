-- Global maintenance ownership is separate from document room/admission fences.
-- Keep rows/generation across release and restart. No workspace FK or event cursor.
CREATE TABLE maintenance_job_claims (
    job_key INTEGER PRIMARY KEY CHECK (job_key BETWEEN 1 AND 9),
    owner_token BLOB CHECK (owner_token IS NULL OR
        (typeof(owner_token)='blob' AND length(owner_token)=16)),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    expires_at INTEGER CHECK (expires_at IS NULL OR expires_at >= 0),
    CHECK ((owner_token IS NULL) = (expires_at IS NULL)),
    CHECK (owner_token IS NULL OR generation > 0)
) STRICT;
INSERT INTO maintenance_job_claims(job_key,generation) VALUES
    (1,0),(2,0),(3,0),(4,0),(5,0),(6,0),(7,0),(8,0),(9,0);
