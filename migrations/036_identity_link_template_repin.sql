-- Identity links that stored a Microsoft tenant-template issuer.
--
-- Before this release a Microsoft `common`/`organizations`/`consumers` sign-in
-- recorded the discovery issuer, which is the literal template
-- `https://login.microsoftonline.com/{tenantid}/v2.0`, instead of the tenant
-- issuer the id_token was verified against. Such a link is treated as not yet
-- pinned to a tenant; its next successful sign-in records the real tenant
-- issuer here, and from then on only that tenant matches. Nothing is guessed
-- or backfilled by this migration, and the 035 function is unchanged.

-- Replaces the issuer only when it is exactly p_template and p_issuer is a
-- tenant instance of it (the template with `{tenantid}` filled by a GUID). The
-- app role has no UPDATE on identity_links. The owner_isolation rule is
-- restated as in 035. Returns true when the link's issuer is p_issuer
-- afterwards (set now or by a concurrent sign-in of the same tenant).
CREATE FUNCTION fvoci.app_identity_link_repin_template(p_id uuid, p_template text, p_issuer text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
DECLARE
    v_at integer := strpos(p_template, '{tenantid}');
    v_prefix text;
    v_suffix text;
    v_tenant text;
BEGIN
    IF v_at = 0 OR p_issuer IS NULL OR p_issuer = p_template THEN
        RETURN false;
    END IF;
    v_prefix := left(p_template, v_at - 1);
    v_suffix := substr(p_template, v_at + length('{tenantid}'));
    IF length(p_issuer) <= length(v_prefix) + length(v_suffix)
        OR left(p_issuer, length(v_prefix)) <> v_prefix
        OR right(p_issuer, length(v_suffix)) <> v_suffix THEN
        RETURN false;
    END IF;
    v_tenant := substr(p_issuer, length(v_prefix) + 1,
                       length(p_issuer) - length(v_prefix) - length(v_suffix));
    IF v_tenant !~ '^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$' THEN
        RETURN false;
    END IF;
    UPDATE fvoci.identity_links
    SET issuer = p_issuer,
        updated_at = now()
    WHERE id = p_id
      AND issuer = p_template
      AND (public.app_system_ctx_on() OR user_id = public.app_self_user_id());
    RETURN EXISTS (
        SELECT 1 FROM fvoci.identity_links
        WHERE id = p_id
          AND issuer = p_issuer
          AND (public.app_system_ctx_on() OR user_id = public.app_self_user_id())
    );
END;
$$;

REVOKE EXECUTE ON FUNCTION fvoci.app_identity_link_repin_template(uuid, text, text) FROM PUBLIC;
