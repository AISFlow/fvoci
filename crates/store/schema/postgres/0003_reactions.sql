-- Reactions: who answered a block (a comment, a card, a page, a unit) with which emoji.
-- A row of an organization, behind the wall like every other; it hangs on its block in
-- the same organization and goes with it, and with the account it names. The emoji is
-- checked by the server (`fvoci_model::Emoji`); the table holds no more than its
-- length.
--
-- The line first: the words of the keys (`ON DELETE`) count as touching rows.
SELECT set_config('fvoci.system', 'on', true);

CREATE TABLE reactions (
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    block_id uuid NOT NULL,
    user_id uuid NOT NULL,
    emoji text NOT NULL,
    created_at bigint NOT NULL,
    CONSTRAINT reactions_emoji_check CHECK (((char_length(emoji) >= 1) AND (char_length(emoji) <= 16)))
);

-- The block first: every read and write asks by it, as of body_versions; the key holds
-- the block's foreign key too.
ALTER TABLE reactions ADD CONSTRAINT reactions_pkey PRIMARY KEY (block_id, org_id, user_id, emoji);

CREATE INDEX reactions_user_id_idx ON reactions USING btree (user_id);

ALTER TABLE reactions ADD CONSTRAINT reactions_block_org_fkey FOREIGN KEY (block_id, org_id) REFERENCES blocks(id, org_id) ON DELETE CASCADE;
ALTER TABLE reactions ADD CONSTRAINT reactions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;

ALTER TABLE reactions ENABLE ROW LEVEL SECURITY;
ALTER TABLE reactions FORCE ROW LEVEL SECURITY;

CREATE POLICY org_wall ON reactions USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));
