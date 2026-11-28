-- The stars a person puts on a board, a wiki, a course, a card, a page or a unit of a
-- workspace: theirs alone, in their order, a hundred a workspace. A star names what it
-- is on by its identifier alone and holds no key to it: whether its person reads that
-- is asked again whenever the stars are listed.
CREATE TABLE favorites (
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    target_id TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('container', 'card', 'page', 'unit')),
    sort_order BIGINT NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, target_id)
);

CREATE INDEX favorites_user_workspace_idx ON favorites (user_id, workspace_id, sort_order);
CREATE INDEX favorites_target_idx ON favorites (target_id);
