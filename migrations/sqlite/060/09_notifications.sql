-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 09: notifications.
-- notifications, preferences, calendar feed tokens, the VAPID instance row and Web Push subscriptions/deliveries.

CREATE TABLE notifications (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    verb TEXT NOT NULL,
    actor_user_id BLOB CHECK (actor_user_id IS NULL OR (typeof(actor_user_id)='blob' AND length(actor_user_id)=16)),
    target_type TEXT,
    target_id BLOB CHECK (target_id IS NULL OR (typeof(target_id)='blob' AND length(target_id)=16)),
    payload TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(payload)),
    read_at INTEGER,
    archived_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id), UNIQUE (workspace_id,user_id,event_id)
) STRICT;
CREATE TABLE notification_prefs (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    in_app INTEGER NOT NULL DEFAULT 1 CHECK (in_app IN (0,1)),
    mail_immediate INTEGER NOT NULL DEFAULT 1 CHECK (mail_immediate IN (0,1)),
    mail_digest INTEGER NOT NULL DEFAULT 0 CHECK (mail_digest IN (0,1)),
    last_digest_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,user_id),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE ics_tokens (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    token_hash TEXT NOT NULL UNIQUE,
    expires_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,user_id),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE instance_config (
    id INT PRIMARY KEY NOT NULL CHECK (id=1),
    vapid_public_key TEXT,
    vapid_private_key TEXT CHECK (vapid_private_key IS NULL OR substr(vapid_private_key,1,7)='enc:v2:')
) STRICT;
CREATE TABLE push_subscriptions (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    endpoint TEXT NOT NULL CHECK (length(CAST(endpoint AS BLOB))<=2048),
    p256dh TEXT NOT NULL CHECK (length(p256dh)=87),
    auth TEXT NOT NULL CHECK (length(auth)=22),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    session_id BLOB NOT NULL REFERENCES sessions(id) ON DELETE CASCADE CHECK (typeof(session_id)='blob' AND length(session_id)=16),
    UNIQUE (user_id,endpoint)
) STRICT;
CREATE TABLE push_deliveries (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    event_id BLOB NOT NULL CHECK (typeof(event_id)='blob' AND length(event_id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    subscription_id BLOB NOT NULL REFERENCES push_subscriptions(id) ON DELETE CASCADE CHECK (typeof(subscription_id)='blob' AND length(subscription_id)=16),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt BETWEEN -2147483648 AND 2147483647),
    claimed_until INTEGER,
    handed_off_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (event_id,subscription_id)
) STRICT;
CREATE INDEX notifications_inbox_idx ON notifications(workspace_id,user_id,created_at,id);
CREATE INDEX notifications_unread_idx ON notifications(workspace_id,user_id) WHERE read_at IS NULL AND archived_at IS NULL;
CREATE INDEX notification_prefs_digest_due_idx ON notification_prefs(last_digest_at) WHERE mail_digest=1;
CREATE INDEX ics_tokens_expires_at_idx ON ics_tokens(expires_at);
CREATE INDEX push_subscriptions_user_updated_idx ON push_subscriptions(user_id,updated_at DESC);
CREATE INDEX push_subscriptions_endpoint_idx ON push_subscriptions(endpoint);
CREATE INDEX push_subscriptions_session_idx ON push_subscriptions(session_id);
CREATE INDEX push_deliveries_subscription_idx ON push_deliveries(subscription_id);
INSERT INTO instance_config(id) VALUES (1);
