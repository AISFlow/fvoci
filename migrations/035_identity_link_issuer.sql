-- Identity links remember the issuer that verified them.
--
-- Numbered 035 because 031, 033 and 034 are reserved by open branches.
--
-- A provider subject (`sub`) is only unique within its issuer. Links made
-- before this migration have no issuer (NULL): nothing is guessed or
-- backfilled here. The next successful sign-in through such a link records
-- the issuer it was verified against (app_identity_link_backfill_issuer);
-- from then on the link only matches that issuer. Changing a workspace's SSO
-- issuer therefore leaves existing links on the old issuer instead of
-- handing them to whoever the new IdP calls by the same `sub`.
--
-- The unique keys stay (provider, provider_user_id) and (user_id, provider).

ALTER TABLE fvoci.identity_links
    ADD COLUMN issuer text,
    ADD CONSTRAINT identity_links_issuer_nonempty_check CHECK (issuer IS NULL OR issuer <> '');

-- Write-once: sets the issuer of a pre-035 link and never overwrites one.
-- The app role has no UPDATE on identity_links. The owner_isolation rule is
-- restated here because a definer owned by a role that bypasses RLS would not
-- see the policy: the caller must be in the system context or be the link's
-- owner. Returns true when the link's issuer is p_issuer afterwards (set now
-- or by a concurrent sign-in).
CREATE FUNCTION fvoci.app_identity_link_backfill_issuer(p_id uuid, p_issuer text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, fvoci, public
AS $$
BEGIN
    UPDATE fvoci.identity_links
    SET issuer = p_issuer,
        updated_at = now()
    WHERE id = p_id
      AND issuer IS NULL
      AND (public.app_system_ctx_on() OR user_id = public.app_self_user_id());
    RETURN EXISTS (
        SELECT 1 FROM fvoci.identity_links
        WHERE id = p_id
          AND issuer = p_issuer
          AND (public.app_system_ctx_on() OR user_id = public.app_self_user_id())
    );
END;
$$;

REVOKE EXECUTE ON FUNCTION fvoci.app_identity_link_backfill_issuer(uuid, text) FROM PUBLIC;
