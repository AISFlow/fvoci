-- The stars a person puts on a board, a wiki, a course, a card, a page or a unit of a
-- workspace: theirs alone, in their order, a hundred a workspace. A star names what it
-- is on by its identifier alone and holds no key to it: whether its person reads that
-- is asked again whenever the stars are listed.
SELECT set_config('fvoci.system', 'on', true);

CREATE TABLE favorites (
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    target_id uuid NOT NULL,
    target_kind text NOT NULL,
    sort_order bigint DEFAULT 0 NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT favorites_target_kind_check CHECK ((target_kind = ANY (ARRAY['container'::text, 'card'::text, 'page'::text, 'unit'::text])))
);

ALTER TABLE favorites ADD CONSTRAINT favorites_pkey PRIMARY KEY (org_id, user_id, target_id);
CREATE INDEX favorites_user_workspace_idx ON favorites USING btree (user_id, workspace_id, sort_order);
CREATE INDEX favorites_target_idx ON favorites USING btree (target_id);
ALTER TABLE favorites ADD CONSTRAINT favorites_workspace_org_fkey FOREIGN KEY (workspace_id, org_id) REFERENCES workspaces(id, org_id) ON DELETE CASCADE;
ALTER TABLE favorites ADD CONSTRAINT favorites_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;

ALTER TABLE favorites ENABLE ROW LEVEL SECURITY;
ALTER TABLE favorites FORCE ROW LEVEL SECURITY;
CREATE POLICY org_wall ON favorites USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));
