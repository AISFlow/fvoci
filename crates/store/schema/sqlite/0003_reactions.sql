-- Reactions: who answered a block (a comment, a card, a page, a unit) with which emoji.
-- A row hangs on its block and goes with it, and with the account it names. The emoji
-- is checked by the server (`fvoci_model::Emoji`); the table holds no more than its
-- length.

CREATE TABLE reactions (
    block_id TEXT NOT NULL REFERENCES blocks (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    emoji TEXT NOT NULL CHECK (length(emoji) BETWEEN 1 AND 16),
    created_at BIGINT NOT NULL,
    PRIMARY KEY (block_id, user_id, emoji)
);

CREATE INDEX reactions_user_id_idx ON reactions (user_id);
