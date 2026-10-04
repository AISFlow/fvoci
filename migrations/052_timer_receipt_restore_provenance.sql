-- A timer command receipt imported by a native archive restore keeps its
-- original request hash and result but must never replay as a live success.
-- NULL for every live write; the restoring import job id otherwise.
ALTER TABLE fvoci.task_timer_commands ADD COLUMN restored_from_archive uuid;
