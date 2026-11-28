-- The activity of cards (`crate::card_events`): who made a card, changed its title, a
-- value of a property, its parent card or whether it is a template, or brought it back
-- from the trash, and when. A row is the card's board's organization's, behind the wall
-- as every such table is. It hangs on no block by a key, as the versions of a body do:
-- it stays while its card is in the trash and goes when the card is gone for good. Who
-- did it is a user while there is one; the erasure of the account takes the identifier
-- and the name of the token away and leaves the row.
--
-- The rows are of no table that had any: the line below is for the check of every
-- migration, which reads the policy's words as touching rows.
SELECT set_config('fvoci.system', 'on', true);

CREATE TABLE card_events (
    id uuid NOT NULL,
    org_id uuid DEFAULT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid NOT NULL,
    container_id uuid NOT NULL,
    card_id uuid NOT NULL,
    actor_id uuid,
    actor_token_name text,
    kind text NOT NULL,
    property_id text,
    before_value jsonb,
    after_value jsonb,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CONSTRAINT card_events_kind_check CHECK ((kind = ANY (ARRAY['created'::text, 'title'::text, 'property'::text, 'parent'::text, 'template'::text, 'restored'::text])))
);

ALTER TABLE card_events ADD CONSTRAINT card_events_pkey PRIMARY KEY (org_id, id);

CREATE INDEX card_events_card_idx ON card_events USING btree (card_id, updated_at DESC, id DESC);

CREATE INDEX card_events_actor_idx ON card_events USING btree (actor_id) WHERE (actor_id IS NOT NULL);

ALTER TABLE card_events ADD CONSTRAINT card_events_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE card_events ADD CONSTRAINT card_events_org_id_fkey FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE card_events ENABLE ROW LEVEL SECURITY;

ALTER TABLE card_events FORCE ROW LEVEL SECURITY;

CREATE POLICY org_wall ON card_events USING (((org_id = ( SELECT (NULLIF(current_setting('fvoci.org'::text, true), ''::text))::uuid AS "nullif")) OR (( SELECT current_setting('fvoci.system'::text, true) AS current_setting) = 'on'::text)));
