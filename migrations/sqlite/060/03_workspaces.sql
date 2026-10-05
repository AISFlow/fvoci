-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 03: workspaces.
-- groups, invitations, API tokens, workspace SSO configuration, holidays and the typed instance settings store.

CREATE TABLE api_tokens (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    user_id BLOB CHECK (user_id IS NULL OR (typeof(user_id)='blob' AND length(user_id)=16)),
    token_hash TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    scopes TEXT NOT NULL CHECK (json_valid(scopes) AND json_type(scopes)='array' AND json_array_length(scopes)>0),
    expires_at INTEGER,
    last_used_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE groups (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    name TEXT NOT NULL CHECK (length(trim(name))>=1 AND length(name)<=100),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    UNIQUE (workspace_id,id)
) STRICT;
CREATE TABLE group_members (
    workspace_id BLOB NOT NULL CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    group_id BLOB NOT NULL CHECK (typeof(group_id)='blob' AND length(group_id)=16),
    user_id BLOB NOT NULL CHECK (typeof(user_id)='blob' AND length(user_id)=16),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    PRIMARY KEY (workspace_id,group_id,user_id),
    FOREIGN KEY (workspace_id,group_id) REFERENCES groups(workspace_id,id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE invitations (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    email TEXT NOT NULL CHECK (length(email)>0 AND email NOT GLOB '*[^!-~]*' AND email=lower(email)),
    role TEXT NOT NULL CHECK (role IN ('owner','admin','member','guest')),
    token_hash TEXT NOT NULL UNIQUE,
    invited_by BLOB NOT NULL REFERENCES users(id) CHECK (typeof(invited_by)='blob' AND length(invited_by)=16),
    expires_at INTEGER NOT NULL,
    accepted_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE workspace_holidays (
    workspace_id BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    date TEXT NOT NULL CHECK (length(date)=10 AND date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
        AND date BETWEEN '0001-01-01' AND '9999-12-31' AND coalesce(date(date,'+0 days')=date,0)),
    PRIMARY KEY (workspace_id,date)
) STRICT;
CREATE TABLE instance_settings (
    key TEXT PRIMARY KEY NOT NULL CHECK (length(key) BETWEEN 1 AND 64 AND key GLOB '[A-Za-z]*' AND key NOT GLOB '*[^A-Za-z0-9.]*'),
    value TEXT NOT NULL CHECK (json_valid(value)),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE TABLE instance_settings_meta (
    id INT PRIMARY KEY NOT NULL DEFAULT 1 CHECK (id=1),
    revision INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE workspace_oidc (
    id BLOB PRIMARY KEY NOT NULL CHECK (typeof(id)='blob' AND length(id)=16),
    workspace_id BLOB NOT NULL UNIQUE REFERENCES workspaces(id) ON DELETE CASCADE CHECK (typeof(workspace_id)='blob' AND length(workspace_id)=16),
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret TEXT NOT NULL CHECK (substr(client_secret,1,7)='enc:v2:'),
    label TEXT NOT NULL DEFAULT 'SSO',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()*1000000 + CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)
) STRICT;
CREATE INDEX api_tokens_workspace_id_user_id_idx ON api_tokens(workspace_id,user_id);
CREATE INDEX group_members_workspace_id_user_id_idx ON group_members(workspace_id,user_id);
CREATE INDEX invitations_workspace_id_invited_by_idx ON invitations(workspace_id,invited_by);
INSERT INTO instance_settings_meta(id,revision) VALUES (1,0);
CREATE TRIGGER api_tokens_scopes_insert BEFORE INSERT ON api_tokens BEGIN
    SELECT RAISE(ABORT,'api token scopes') WHERE EXISTS (SELECT 1 FROM json_each(NEW.scopes)
        WHERE type<>'text' OR value NOT IN ('documents.read','documents.write','tasks.read','tasks.write','projects.read','projects.manage','share.manage','workspace.manage'));
END;
CREATE TRIGGER api_tokens_scopes_update BEFORE UPDATE OF scopes ON api_tokens BEGIN
    SELECT RAISE(ABORT,'api token scopes') WHERE EXISTS (SELECT 1 FROM json_each(NEW.scopes)
        WHERE type<>'text' OR value NOT IN ('documents.read','documents.write','tasks.read','tasks.write','projects.read','projects.manage','share.manage','workspace.manage'));
END;
