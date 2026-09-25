-- Workspace integrations: outgoing webhooks and the GitHub App link.
--
-- webhooks.secret holds the signing secret sealed at rest
-- (enc:v2:<kid>:<base64url(iv||tag||ciphertext)>, AAD bound to the row), never
-- plaintext. webhook_deliveries is the per-target retry ledger fanned out by the
-- `webhooks` outbox consumer; the same consumer sends due rows under its lease.
-- github_deliveries dedupes inbound GitHub deliveries by X-GitHub-Delivery.

CREATE TABLE fvoci.webhooks (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    url text NOT NULL,
    secret text NOT NULL,
    events text[] NOT NULL,
    created_by uuid NOT NULL REFERENCES fvoci.users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT webhooks_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT webhooks_events_check CHECK (cardinality(events) BETWEEN 1 AND 64),
    CONSTRAINT webhooks_url_check CHECK (char_length(url) BETWEEN 1 AND 2048),
    CONSTRAINT webhooks_secret_check CHECK (secret LIKE 'enc:v2:%')
);

CREATE INDEX webhooks_created_by_idx ON fvoci.webhooks (created_by);

ALTER TABLE fvoci.webhooks ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.webhooks FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.webhooks
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.webhook_deliveries (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    webhook_id uuid NOT NULL,
    event_id uuid NOT NULL,
    attempt integer NOT NULL DEFAULT 0,
    status text NOT NULL,
    http_status integer,
    next_attempt_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT webhook_deliveries_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT webhook_deliveries_webhook_event_unique UNIQUE (webhook_id, event_id),
    CONSTRAINT webhook_deliveries_status_check CHECK (status IN ('pending', 'delivered', 'failed')),
    CONSTRAINT webhook_deliveries_attempt_check CHECK (attempt >= 0),
    CONSTRAINT webhook_deliveries_pending_check
        CHECK ((status = 'pending') = (next_attempt_at IS NOT NULL)),
    CONSTRAINT webhook_deliveries_workspace_webhook_fk
        FOREIGN KEY (workspace_id, webhook_id)
        REFERENCES fvoci.webhooks (workspace_id, id) ON DELETE CASCADE
);

CREATE INDEX webhook_deliveries_pending_next_attempt_at_idx
    ON fvoci.webhook_deliveries (next_attempt_at)
    WHERE status = 'pending';
CREATE INDEX webhook_deliveries_settled_created_at_idx
    ON fvoci.webhook_deliveries (created_at)
    WHERE status IN ('delivered', 'failed');

ALTER TABLE fvoci.webhook_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.webhook_deliveries FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.webhook_deliveries
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.github_installations (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    installation_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT github_installations_installation_id_unique UNIQUE (installation_id),
    CONSTRAINT github_installations_workspace_id_unique UNIQUE (workspace_id),
    CONSTRAINT github_installations_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT github_installations_installation_id_check
        CHECK (installation_id ~ '^[0-9]{1,20}$')
);

ALTER TABLE fvoci.github_installations ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.github_installations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.github_installations
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

-- Pending install round trips: the hash of the nonce carried in the signed
-- state, bound to the admin user and session that started it. The callback
-- deletes the row (single use) and must present the same session.
CREATE TABLE fvoci.github_install_states (
    nonce_hash text PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES fvoci.users (id) ON DELETE CASCADE,
    session_id uuid NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT github_install_states_nonce_hash_check CHECK (nonce_hash ~ '^[0-9a-f]{64}$')
);

CREATE INDEX github_install_states_workspace_id_idx ON fvoci.github_install_states (workspace_id);
CREATE INDEX github_install_states_expires_at_idx ON fvoci.github_install_states (expires_at);

ALTER TABLE fvoci.github_install_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.github_install_states FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON fvoci.github_install_states
    AS PERMISSIVE FOR ALL TO public
    USING (
        workspace_id = (SELECT public.app_tenant_id())
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.github_issue_links (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    repo text NOT NULL,
    issue_number integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT github_issue_links_workspace_id_id_unique UNIQUE (workspace_id, id),
    CONSTRAINT github_issue_links_task_unique UNIQUE (workspace_id, task_id),
    CONSTRAINT github_issue_links_issue_unique UNIQUE (workspace_id, repo, issue_number),
    CONSTRAINT github_issue_links_issue_number_check CHECK (issue_number > 0),
    CONSTRAINT github_issue_links_repo_check CHECK (char_length(repo) BETWEEN 3 AND 200),
    CONSTRAINT github_issue_links_workspace_task_fk
        FOREIGN KEY (workspace_id, task_id)
        REFERENCES fvoci.tasks (workspace_id, id) ON DELETE CASCADE
);

ALTER TABLE fvoci.github_issue_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.github_issue_links FORCE ROW LEVEL SECURITY;
-- Source policy has no system bypass: every access is tenant scoped.
CREATE POLICY tenant_isolation ON fvoci.github_issue_links
    AS PERMISSIVE FOR ALL TO public
    USING (workspace_id = (SELECT public.app_tenant_id()));

CREATE TABLE fvoci.github_deliveries (
    delivery_id uuid PRIMARY KEY,
    processed_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX github_deliveries_processed_at_idx ON fvoci.github_deliveries (processed_at);

ALTER TABLE fvoci.github_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.github_deliveries FORCE ROW LEVEL SECURITY;
CREATE POLICY system_only ON fvoci.github_deliveries
    AS PERMISSIVE FOR ALL TO public
    USING ((SELECT public.app_system_ctx_on()));

-- Start both consumers after the events already recorded, as 018/020 do, so an
-- upgrade does not scan (or deliver) history recorded before these features.
DO $$
BEGIN
    PERFORM pg_catalog.set_config('app.system_ctx', 'on', true);
    INSERT INTO fvoci.outbox_consumers (consumer, last_xact, last_seq)
    SELECT c.name, e.xact, e.seq
    FROM (VALUES ('webhooks'), ('github')) AS c (name)
    CROSS JOIN LATERAL (
        SELECT ev.xact, ev.seq
        FROM fvoci.events AS ev
        WHERE ev.xact < pg_catalog.pg_snapshot_xmin(pg_catalog.pg_current_snapshot())
        ORDER BY ev.xact DESC, ev.seq DESC
        LIMIT 1
    ) AS e
    ON CONFLICT (consumer) DO NOTHING;
    PERFORM pg_catalog.set_config('app.system_ctx', '', true);
END;
$$;
