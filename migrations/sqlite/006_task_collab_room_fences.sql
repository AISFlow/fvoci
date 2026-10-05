-- Task room lineage uses the existing step003 global monotonic counter.
-- A document room lease can never certify a task writer. Allocation, DB time,
-- native generation and authorization stay in the same reserved writer.
CREATE TABLE task_collab_room_fences (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    task_id BLOB NOT NULL CHECK (typeof(task_id)='blob' AND length(task_id)=16),
    owner_token BLOB NOT NULL CHECK (typeof(owner_token)='blob' AND length(owner_token)=16),
    fence INTEGER NOT NULL CHECK (typeof(fence)='integer' AND fence>0),
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id,task_id),
    FOREIGN KEY (workspace_id,task_id) REFERENCES tasks(workspace_id,id) ON DELETE CASCADE
) STRICT;
