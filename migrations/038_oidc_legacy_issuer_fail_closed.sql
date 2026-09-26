-- Legacy identity links have no verified issuer provenance. Leave every row
-- unchanged; authenticated unlink and reconnect creates a new, verified link.
-- Remove the obsolete SECURITY DEFINER paths that could assign an issuer to
-- a NULL or Microsoft template link during sign-in.
DROP FUNCTION fvoci.app_identity_link_backfill_issuer(uuid, text);
DROP FUNCTION fvoci.app_identity_link_repin_template(uuid, text, text);
