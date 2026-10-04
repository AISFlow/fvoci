-- Command run IDs are immutable actor-private historical provenance.
-- Canonical run/segment/task FKs, RLS, grants and triggers remain unchanged.
-- Deploy only with current replay retirement checks and the native restore
-- prior-provenance guard, including archives omitting command/audit history.
-- Never reconstruct previously NULL locators from stored result JSON.
ALTER TABLE fvoci.task_timer_commands
    DROP CONSTRAINT task_timer_commands_run_id_fkey;
