CREATE FUNCTION public.app_self_user_id()
RETURNS uuid
LANGUAGE sql
STABLE
SET search_path = pg_catalog
AS $$ SELECT NULLIF(pg_catalog.current_setting('app.self_user_id', true), '')::uuid $$;

CREATE POLICY memberships_select_self ON fvoci.memberships
    AS PERMISSIVE FOR SELECT TO public
    USING (user_id = (SELECT public.app_self_user_id()));
