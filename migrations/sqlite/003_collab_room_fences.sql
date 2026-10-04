-- SQLite-family step 3: coordinator/W1 authoritative room lease contract.
-- No PostgreSQL counterpart or invented PG migration receipt.
-- Allocation, authoritative DB time, ownership checks and native append fences
-- are named Rust operations in the same BEGIN IMMEDIATE transaction.
CREATE TABLE collab_fence_counter (
    id INT PRIMARY KEY NOT NULL CHECK (id=1),
    next_fence INTEGER NOT NULL CHECK (typeof(next_fence)='integer' AND next_fence>0)
) STRICT;
INSERT INTO collab_fence_counter(id,next_fence) VALUES (1,1);
-- Retiring a room/target cannot recycle a fence. The global counter survives.
CREATE TRIGGER collab_fence_counter_no_delete BEFORE DELETE ON collab_fence_counter BEGIN
    SELECT RAISE(ABORT,'collab fence counter cannot be deleted');
END;
CREATE TRIGGER collab_fence_counter_no_reset BEFORE UPDATE ON collab_fence_counter
WHEN NEW.id<>OLD.id OR NEW.next_fence<OLD.next_fence BEGIN
    SELECT RAISE(ABORT,'collab fence counter cannot be reset');
END;
CREATE TABLE collab_room_fences (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    document_id BLOB NOT NULL CHECK (typeof(document_id)='blob' AND length(document_id)=16),
    owner_token BLOB NOT NULL CHECK (typeof(owner_token)='blob' AND length(owner_token)=16),
    fence INTEGER NOT NULL CHECK (typeof(fence)='integer' AND fence>0),
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id,document_id),
    FOREIGN KEY (workspace_id,document_id) REFERENCES documents(workspace_id,id) ON DELETE CASCADE
) STRICT;
-- Also refuse INSERT OR REPLACE, even with recursive_triggers disabled.
CREATE TRIGGER collab_fence_counter_no_replace BEFORE INSERT ON collab_fence_counter
WHEN EXISTS (SELECT 1 FROM collab_fence_counter) BEGIN
    SELECT RAISE(ABORT,'collab fence counter already exists');
END;
