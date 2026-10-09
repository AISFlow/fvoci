-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 04: events.
-- the event log and its monotonic sequence, the audit log and the outbox relay ledger (unseeded on a fresh install).

CREATE TABLE events (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    seq INTEGER NOT NULL UNIQUE CHECK (typeof(seq)='integer' AND seq>0),
    workspace_id BLOB CHECK (workspace_id IS NULL OR (typeof(workspace_id)='blob' AND length(workspace_id)=16)),
    actor_user_id BLOB CHECK (actor_user_id IS NULL OR (typeof(actor_user_id)='blob' AND length(actor_user_id)=16)),
    verb TEXT NOT NULL,
    target_type TEXT,
    target_id BLOB CHECK (target_id IS NULL OR (typeof(target_id)='blob' AND length(target_id)=16)),
    payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload)),
    channel TEXT NOT NULL DEFAULT 'web' CHECK (channel IN ('web','api','mcp','webhook','system')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE audit_log (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    actor_user_id BLOB CHECK (actor_user_id IS NULL OR (typeof(actor_user_id)='blob' AND length(actor_user_id)=16)),
    workspace_id BLOB CHECK (workspace_id IS NULL OR (typeof(workspace_id)='blob' AND length(workspace_id)=16)),
    verb TEXT NOT NULL,
    target_type TEXT,
    target_id BLOB CHECK (target_id IS NULL OR (typeof(target_id)='blob' AND length(target_id)=16)),
    payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload)),
    ip TEXT, -- checked inet codec in the consumer; no SQLite IP parser
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE outbox_consumers (
    consumer TEXT PRIMARY KEY NOT NULL CHECK (length(consumer) BETWEEN 1 AND 63 AND consumer GLOB '[a-z]*' AND consumer NOT GLOB '*[^a-z0-9_-]*'),
    last_seq INTEGER NOT NULL DEFAULT 0 CHECK (typeof(last_seq)='integer' AND last_seq>=0),
    lease_owner BLOB CHECK (lease_owner IS NULL OR (typeof(lease_owner)='blob' AND length(lease_owner)=16)),
    lease_until INTEGER,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    CHECK ((lease_owner IS NULL)=(lease_until IS NULL))
) STRICT;
CREATE TABLE outbox_failures (
    consumer TEXT NOT NULL,
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    attempts INTEGER NOT NULL CHECK (attempts BETWEEN 1 AND 2147483647),
    last_error TEXT NOT NULL,
    next_attempt_at INTEGER NOT NULL,
    dead_at INTEGER,
    skipped_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (consumer,event_id)
) STRICT;
CREATE TABLE processed_events (
    consumer TEXT NOT NULL,
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    processed_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (consumer,event_id)
) STRICT;
CREATE TABLE event_sequence (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id=1),
    last_seq INTEGER NOT NULL CHECK (typeof(last_seq)='integer' AND last_seq>=0)
) STRICT;
CREATE INDEX events_relay_idx ON events(seq);
CREATE INDEX events_workspace_idx ON events(workspace_id,created_at);
CREATE INDEX events_workspace_relay_idx ON events(workspace_id,seq);
CREATE INDEX audit_log_created_at_id_idx ON audit_log(created_at DESC,id DESC);
CREATE INDEX outbox_failures_retry_idx ON outbox_failures(consumer,next_attempt_at) WHERE dead_at IS NULL;
INSERT INTO event_sequence(id,last_seq) VALUES (1,0);
CREATE TRIGGER event_sequence_no_delete BEFORE DELETE ON event_sequence BEGIN
    SELECT RAISE(ABORT,'event sequence cannot be deleted');
END;
CREATE TRIGGER event_sequence_no_reset BEFORE UPDATE ON event_sequence
WHEN NEW.id<>OLD.id OR NEW.last_seq<OLD.last_seq BEGIN
    SELECT RAISE(ABORT,'event sequence cannot be reset');
END;
CREATE TRIGGER event_sequence_no_replace BEFORE INSERT ON event_sequence
WHEN EXISTS (SELECT 1 FROM event_sequence) BEGIN
    SELECT RAISE(ABORT,'event sequence already exists');
END;
