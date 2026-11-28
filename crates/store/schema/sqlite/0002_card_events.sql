-- The activity of cards (`crate::card_events`), as PostgreSQL's file of this migration
-- has it but for the wall: who made a card, changed its title, a value of a property,
-- its parent card or whether it is a template, or brought it back from the trash, and
-- when. It hangs on no block by a key, as the versions of a body do. A database holds
-- one organization (the baseline's triggers), so a row names none.

CREATE TABLE card_events (
    id TEXT NOT NULL PRIMARY KEY,
    container_id TEXT NOT NULL,
    card_id TEXT NOT NULL,
    actor_id TEXT REFERENCES users (id) ON DELETE SET NULL,
    actor_token_name TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('created', 'title', 'property', 'parent', 'template', 'restored')),
    property_id TEXT,
    before_value TEXT,
    after_value TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE INDEX card_events_card_idx ON card_events (card_id, updated_at DESC, id DESC);
CREATE INDEX card_events_actor_idx ON card_events (actor_id) WHERE actor_id IS NOT NULL;
