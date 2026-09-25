CREATE TABLE fvoci.notifications (
    id uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
    workspace_id uuid NOT NULL REFERENCES fvoci.workspaces (id) ON DELETE CASCADE,
    user_id uuid NOT NULL,
    event_id uuid NOT NULL,
    verb text NOT NULL,
    actor_user_id uuid,
    target_type text,
    target_id uuid,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    read_at timestamptz,
    archived_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT notifications_workspace_id_id_unique UNIQUE (workspace_id, id)
);

CREATE INDEX notifications_inbox_idx
    ON fvoci.notifications (workspace_id, user_id, created_at, id);

CREATE INDEX notifications_unread_idx
    ON fvoci.notifications (workspace_id, user_id)
    WHERE read_at IS NULL AND archived_at IS NULL;

ALTER TABLE fvoci.notifications ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.notifications FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_isolation ON fvoci.notifications
    AS PERMISSIVE FOR ALL TO public
    USING (
        (
            workspace_id = (SELECT public.app_tenant_id())
            AND user_id = (SELECT public.app_self_user_id())
        )
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        (
            workspace_id = (SELECT public.app_tenant_id())
            AND user_id = (SELECT public.app_self_user_id())
        )
        OR (SELECT public.app_system_ctx_on())
    );

CREATE TABLE fvoci.notification_prefs (
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    in_app boolean NOT NULL DEFAULT true,
    mail_immediate boolean NOT NULL DEFAULT true,
    mail_digest boolean NOT NULL DEFAULT false,
    last_digest_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id),
    CONSTRAINT notification_prefs_workspace_user_fk
        FOREIGN KEY (workspace_id, user_id)
        REFERENCES fvoci.memberships (workspace_id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX notification_prefs_digest_due_idx
    ON fvoci.notification_prefs (last_digest_at)
    WHERE mail_digest = true;

ALTER TABLE fvoci.notification_prefs ENABLE ROW LEVEL SECURITY;
ALTER TABLE fvoci.notification_prefs FORCE ROW LEVEL SECURITY;
CREATE POLICY owner_isolation ON fvoci.notification_prefs
    AS PERMISSIVE FOR ALL TO public
    USING (
        (
            workspace_id = (SELECT public.app_tenant_id())
            AND user_id = (SELECT public.app_self_user_id())
        )
        OR (SELECT public.app_system_ctx_on())
    )
    WITH CHECK (
        (
            workspace_id = (SELECT public.app_tenant_id())
            AND user_id = (SELECT public.app_self_user_id())
        )
        OR (SELECT public.app_system_ctx_on())
    );
