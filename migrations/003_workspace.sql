CREATE OR REPLACE FUNCTION public.app_self_user_id()
RETURNS uuid
LANGUAGE sql
STABLE
SET search_path = pg_catalog
AS $$ SELECT NULLIF(pg_catalog.current_setting('app.self_user_id', true), '')::uuid $$;

CREATE POLICY memberships_select_self ON fvoci.memberships
    AS PERMISSIVE FOR SELECT TO public
    USING (user_id = (SELECT public.app_self_user_id()));

ALTER TABLE fvoci.users
    ADD CONSTRAINT users_personal_workspace_fk
    FOREIGN KEY (personal_workspace_id)
    REFERENCES fvoci.workspaces (id)
    ON DELETE SET NULL;

CREATE UNIQUE INDEX users_personal_workspace_id_unique
    ON fvoci.users (personal_workspace_id)
    WHERE personal_workspace_id IS NOT NULL;
