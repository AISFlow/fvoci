use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

const MIGRATIONS: &[(&str, i32)] = &[
    (include_str!("../../migrations/001_schema.sql"), 1),
    (include_str!("../../migrations/002_functions.sql"), 2),
    (include_str!("../../migrations/003_workspace.sql"), 3),
    (include_str!("../../migrations/004_documents.sql"), 4),
    (include_str!("../../migrations/005_collab_updates.sql"), 5),
    (include_str!("../../migrations/006_attachments.sql"), 6),
    (
        include_str!("../../migrations/007_attachment_extract.sql"),
        7,
    ),
    (include_str!("../../migrations/008_projects.sql"), 8),
    (include_str!("../../migrations/009_invitations.sql"), 9),
    (include_str!("../../migrations/010_revisions.sql"), 10),
    (include_str!("../../migrations/011_comments.sql"), 11),
    (include_str!("../../migrations/012_api_tokens.sql"), 12),
    (include_str!("../../migrations/013_outbox.sql"), 13),
    (include_str!("../../migrations/014_search_index.sql"), 14),
    (include_str!("../../migrations/015_task_labels.sql"), 15),
    (include_str!("../../migrations/016_groups.sql"), 16),
    (include_str!("../../migrations/017_task_milestones.sql"), 17),
    (include_str!("../../migrations/018_notifications.sql"), 18),
    (include_str!("../../migrations/019_schedule_ics.sql"), 19),
    (include_str!("../../migrations/020_mail_reset.sql"), 20),
    (include_str!("../../migrations/021_maintenance_gc.sql"), 21),
    (include_str!("../../migrations/022_task_activity.sql"), 22),
    (include_str!("../../migrations/023_import_jobs.sql"), 23),
    (include_str!("../../migrations/024_share_stars.sql"), 24),
    (
        include_str!("../../migrations/025_account_lifecycle.sql"),
        25,
    ),
    (include_str!("../../migrations/026_admin_console.sql"), 26),
    (include_str!("../../migrations/027_integrations.sql"), 27),
    (
        include_str!("../../migrations/028_collections_views.sql"),
        28,
    ),
    (include_str!("../../migrations/029_mfa_oidc.sql"), 29),
    (
        include_str!("../../migrations/030_attachments_complete.sql"),
        30,
    ),
    // 031 is reserved by an open branch.
    (
        include_str!("../../migrations/031_attachment_embeddings.sql"),
        31,
    ),
    (
        include_str!("../../migrations/032_admin_user_erase.sql"),
        32,
    ),
    (
        include_str!("../../migrations/033_import_deferred_events.sql"),
        33,
    ),
    (
        include_str!("../../migrations/034_task_time_entries.sql"),
        34,
    ),
    (
        include_str!("../../migrations/035_identity_link_issuer.sql"),
        35,
    ),
    (
        include_str!("../../migrations/036_identity_link_template_repin.sql"),
        36,
    ),
    (include_str!("../../migrations/037_task_collab.sql"), 37),
    (
        include_str!("../../migrations/038_oidc_legacy_issuer_fail_closed.sql"),
        38,
    ),
    (include_str!("../../migrations/039_templates.sql"), 39),
    (include_str!("../../migrations/040_web_push.sql"), 40),
    (
        include_str!("../../migrations/041_outbox_consumer_seed_repair.sql"),
        41,
    ),
    (
        include_str!("../../migrations/042_secret_maintenance.sql"),
        42,
    ),
    (
        include_str!("../../migrations/043_events_index_outbox_lag.sql"),
        43,
    ),
    (
        include_str!("../../migrations/044_email_change_auth_generation.sql"),
        44,
    ),
    (
        include_str!("../../migrations/045_personal_input_commands.sql"),
        45,
    ),
    (include_str!("../../migrations/046_native_archives.sql"), 46),
    (
        include_str!("../../migrations/047_personal_transfer_commands.sql"),
        47,
    ),
    (include_str!("../../migrations/048_task_timers.sql"), 48),
    (
        include_str!("../../migrations/049_task_estimate_unit.sql"),
        49,
    ),
    (
        include_str!("../../migrations/050_revision_restore_metadata.sql"),
        50,
    ),
    (include_str!("../../migrations/051_zotero_readonly.sql"), 51),
    (
        include_str!("../../migrations/052_timer_receipt_restore_provenance.sql"),
        52,
    ),
    (
        include_str!("../../migrations/053_timer_receipt_historical_run.sql"),
        53,
    ),
    (
        include_str!("../../migrations/054_timer_audit_actor_index.sql"),
        54,
    ),
    (
        include_str!("../../migrations/055_wiki_create_commands.sql"),
        55,
    ),
];

pub(crate) const MIGRATION_LOCK_KEY: i64 = 847_291_003_552;

const APP_ROLE_GRANTS: &str = include_str!("../../scripts/grant-app-role.sql");
const APP_ROLE_PLACEHOLDER: &str = ":\"app_role\"";

pub const SCHEMA_GATE_OPERATOR_HINT: &str =
    "run `fvoci-migrate` then `fvoci-migrate --grant-app-role <app-role>` before starting fvoci-server";

/// Number of migrations compiled into this binary. Versions may have gaps
/// (a number reserved by an unmerged branch), so this is not the latest version.
pub fn compiled_migration_count() -> usize {
    MIGRATIONS.len()
}

/// Latest migration version compiled into this binary.
pub fn latest_migration_version() -> i32 {
    MIGRATIONS.last().map(|(_, version)| *version).unwrap_or(0)
}

/// Every migration version compiled into this binary, ascending.
pub fn compiled_migration_versions() -> Vec<i32> {
    MIGRATIONS.iter().map(|(_, version)| *version).collect()
}

/// Compares the applied migration set with the compiled one. Every compiled
/// version must be applied and nothing else may be: a version number that
/// merges after a higher one (031 after 032) is still required, so the check
/// is on the set, not on `max(version)`.
pub fn schema_gate(applied: &[i32], compiled: &[i32]) -> Result<(), String> {
    let expected = compiled.last().copied().unwrap_or(0);
    if applied.is_empty() {
        return Err(format!(
            "database has no applied migrations (expected version {expected}); {SCHEMA_GATE_OPERATOR_HINT}"
        ));
    }
    let unknown: Vec<i32> = applied
        .iter()
        .copied()
        .filter(|version| !compiled.contains(version))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "database schema has migrations {unknown:?} newer than this binary ({expected}); deploy a matching fvoci-server"
        ));
    }
    let missing: Vec<i32> = compiled
        .iter()
        .copied()
        .filter(|version| !applied.contains(version))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "database schema is missing migrations {missing:?} and is behind compiled version {expected}; {SCHEMA_GATE_OPERATOR_HINT}"
        ));
    }
    Ok(())
}

/// Verifies the connected database matches the compiled migration set.
pub async fn assert_schema_current(pool: &PgPool) -> Result<(), String> {
    let applied = sqlx::query_scalar::<_, i32>(
        "SELECT version FROM fvoci.schema_migrations ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| {
        format!("cannot read fvoci.schema_migrations ({error}); {SCHEMA_GATE_OPERATOR_HINT}")
    })?;
    schema_gate(&applied, &compiled_migration_versions())
}

// PostgreSQL grants EXECUTE to PUBLIC when a function is created. Revoke it for
// the migration owner's SECURITY DEFINER functions inside the same transaction
// that created them, so no window exists before grant-app-role.sql runs.
const REVOKE_PUBLIC_DEFINER_EXECUTE: &str = r#"
DO $$
DECLARE
    definer regprocedure;
BEGIN
    FOR definer IN
        SELECT p.oid::regprocedure
        FROM pg_proc p
        INNER JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE (n.nspname = 'fvoci' OR (n.nspname = 'public' AND p.proname LIKE 'app\_%'))
          AND p.prosecdef
          AND p.proowner = (SELECT oid FROM pg_roles WHERE rolname = current_user)
    LOOP
        EXECUTE format('REVOKE EXECUTE ON ROUTINE %s FROM PUBLIC', definer);
    END LOOP;
END
$$;
"#;

// Migrations 016, 018 and 040 call unqualified `uuidv7()`, a built-in only
// from PostgreSQL 18. On 16 and 17 this creates `public.uuidv7()` with the
// RFC 9562 section 5.7 layout: 48-bit Unix milliseconds from
// clock_timestamp(), version 7, and the RFC variant and remaining 74 bits
// taken from built-in gen_random_uuid() (CSPRNG). A clock outside the
// 48-bit millisecond range raises instead of truncating. PG18's per-backend
// sub-millisecond monotonicity is not reproduced: ids created in the same
// millisecond have no defined relative order, so ordering by id is only
// coarsely FIFO. An existing `uuidv7` on the migration search path is reused
// only when it is exactly this function owned by the migration role;
// anything else fails before it is changed. PostgreSQL 18 is left untouched.
const UUIDV7_COMPAT_PREFLIGHT: &str = r#"
DO $preflight$
DECLARE
    caller_schemas pg_catalog.name[] := pg_catalog.current_schemas(true);
    shim_body pg_catalog.text := $body$
DECLARE
    unix_ms pg_catalog.int8 := pg_catalog.floor(
        pg_catalog.extract('epoch', pg_catalog.clock_timestamp()) * 1000
    );
BEGIN
    IF unix_ms < 0 OR unix_ms > 281474976710655 THEN
        RAISE EXCEPTION 'uuidv7: clock % ms is outside the 48-bit Unix millisecond range', unix_ms;
    END IF;
    RETURN pg_catalog.encode(
        pg_catalog.set_bit(
            pg_catalog.set_bit(
                pg_catalog.overlay(
                    pg_catalog.uuid_send(pg_catalog.gen_random_uuid()),
                    pg_catalog.substr(pg_catalog.int8send(unix_ms), 3),
                    1,
                    6
                ),
                52,
                1
            ),
            53,
            1
        ),
        'hex'
    )::pg_catalog.uuid;
END
$body$;
    shim_config pg_catalog.text[] := ARRAY['search_path=pg_catalog, pg_temp'];
    existing record;
    found_shim boolean := false;
BEGIN
    PERFORM pg_catalog.set_config('search_path', 'pg_catalog, pg_temp', true);

    IF pg_catalog.current_setting('server_version_num')::integer >= 180000 THEN
        RETURN;
    END IF;

    IF NOT 'public' = ANY (caller_schemas) THEN
        RAISE EXCEPTION 'uuidv7 compatibility: schema "public" is not on the migration search_path (%)',
            pg_catalog.array_to_string(caller_schemas, ', ')
            USING HINT = 'run fvoci-migrate with the default search_path so migrations resolve public.uuidv7()';
    END IF;

    FOR existing IN
        SELECT p.oid::regprocedure AS signature,
               n.nspname = 'public' AND p.pronargs = 0 AS is_candidate,
               p.proowner = (SELECT r.oid FROM pg_roles r WHERE r.rolname = current_user) AS owned,
               pg_get_userbyid(p.proowner) AS owner_name,
               p.prokind = 'f'
                   AND p.prolang = (SELECT l.oid FROM pg_language l WHERE l.lanname = 'plpgsql')
                   AND p.prosrc = shim_body
                   AND p.prorettype = 'pg_catalog.uuid'::regtype
                   AND NOT p.proretset
                   AND p.provolatile = 'v'
                   AND p.proparallel = 's'
                   AND NOT p.prosecdef
                   AND NOT p.proleakproof
                   AND NOT p.proisstrict
                   AND p.pronargdefaults = 0
                   AND p.proconfig IS NOT DISTINCT FROM shim_config AS exact
        FROM pg_proc p
        INNER JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE p.proname = 'uuidv7'
          AND n.nspname = ANY (caller_schemas || 'public'::name)
        ORDER BY n.nspname, p.oid
    LOOP
        IF existing.is_candidate AND existing.owned AND existing.exact THEN
            found_shim := true;
        ELSE
            RAISE EXCEPTION 'uuidv7 compatibility: existing function % (owner %) is not the FVOCI shim owned by %',
                existing.signature, existing.owner_name, current_user
                USING HINT = 'drop or rename that function (or remove its schema from the migration search_path) and rerun fvoci-migrate; it was not changed';
        END IF;
    END LOOP;

    IF NOT found_shim THEN
        EXECUTE format(
            'CREATE FUNCTION public.uuidv7() RETURNS pg_catalog.uuid LANGUAGE plpgsql VOLATILE PARALLEL SAFE SET search_path = pg_catalog, pg_temp AS %L',
            shim_body
        );
    END IF;
END
$preflight$;
"#;

/// Runs the version-specific preflight under the migration lock in its own
/// transaction, before any migration is applied.
async fn preflight(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql(UUIDV7_COMPAT_PREFLIGHT)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn run_migrations(url: &str) -> Result<(), sqlx::Error> {
    run_migrations_through(url, i32::MAX).await
}

pub async fn run_migrations_through(url: &str, max_version: i32) -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new().max_connections(2).connect(url).await?;
    let result = apply_migrations(&pool, max_version).await;
    pool.close().await;
    result
}

async fn apply_migrations(pool: &PgPool, max_version: i32) -> Result<(), sqlx::Error> {
    preflight(pool).await?;
    for (sql, version) in MIGRATIONS {
        if *version > max_version {
            continue;
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *tx)
            .await?;

        let bootstrapped: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'fvoci' AND table_name = 'schema_migrations'
            )",
        )
        .fetch_one(&mut *tx)
        .await?;

        if bootstrapped {
            let applied: Option<i32> = sqlx::query_scalar(
                "SELECT version FROM fvoci.schema_migrations WHERE version = $1",
            )
            .bind(version)
            .fetch_optional(&mut *tx)
            .await?;
            if applied.is_some() {
                tx.rollback().await?;
                continue;
            }
        }

        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::raw_sql(REVOKE_PUBLIC_DEFINER_EXECUTE)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO fvoci.schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(version)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    Ok(())
}

#[derive(Debug)]
pub enum GrantError {
    Db(sqlx::Error),
    InvalidRole(String),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(error) => write!(
                f,
                "app role grant failed; no privileges were committed: {error}"
            ),
            Self::InvalidRole(reason) => write!(f, "app role grant refused: {reason}"),
        }
    }
}

impl std::error::Error for GrantError {}

impl From<sqlx::Error> for GrantError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error)
    }
}

/// Returns scripts/grant-app-role.sql with the psql `:"app_role"` variable
/// replaced by a quoted identifier.
pub fn app_role_grant_sql(role: &str) -> String {
    let quoted = format!("\"{}\"", role.replace('"', "\"\""));
    APP_ROLE_GRANTS.replace(APP_ROLE_PLACEHOLDER, &quoted)
}

/// Applies scripts/grant-app-role.sql for `role` as one transaction through the
/// migration owner URL. Any failure rolls back every statement, so the broad
/// table grant never survives without the narrowing revokes that follow it.
pub async fn grant_app_role(url: &str, role: &str) -> Result<(), GrantError> {
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    let result = apply_app_role_grants(&pool, role).await;
    pool.close().await;
    result
}

pub async fn apply_app_role_grants(pool: &PgPool, role: &str) -> Result<(), GrantError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let found: Option<(bool, bool)> =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = $1")
            .bind(role)
            .fetch_optional(&mut *tx)
            .await?;
    match found {
        None => {
            return Err(GrantError::InvalidRole(format!(
                "role {role:?} does not exist"
            )))
        }
        Some((true, _)) | Some((_, true)) => {
            return Err(GrantError::InvalidRole(format!(
                "role {role:?} must be non-superuser without BYPASSRLS"
            )))
        }
        Some(_) => {}
    }
    // The script revokes privileges from `role`; applied to an owner it would
    // strip the ACL that the SECURITY DEFINER functions run under.
    let owns_schema_objects: bool = sqlx::query_scalar(
        r#"
        SELECT $1 = current_user::text
            OR pg_has_role($1, n.nspowner, 'MEMBER')
            OR EXISTS (
                SELECT 1 FROM pg_class c
                WHERE c.relnamespace = n.oid AND pg_has_role($1, c.relowner, 'MEMBER')
            )
            OR EXISTS (
                SELECT 1 FROM pg_proc p
                INNER JOIN pg_namespace pn ON pn.oid = p.pronamespace
                WHERE (pn.oid = n.oid OR (pn.nspname = 'public' AND p.proname LIKE 'app\_%'))
                  AND pg_has_role($1, p.proowner, 'MEMBER')
            )
        FROM pg_namespace n
        WHERE n.nspname = 'fvoci'
        "#,
    )
    .bind(role)
    .fetch_one(&mut *tx)
    .await?;
    if owns_schema_objects {
        return Err(GrantError::InvalidRole(format!(
            "role {role:?} owns or inherits ownership of fvoci objects; use a separate app role"
        )));
    }
    sqlx::raw_sql(&app_role_grant_sql(role))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn assert_app_role(pool: &PgPool) -> Result<(), String> {
    let row: (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())?;
    if row.0 || row.1 {
        return Err("DATABASE_APP_URL must use a non-superuser role without BYPASSRLS".into());
    }

    let owns_schema: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_namespace n
            WHERE n.nspname = 'fvoci'
              AND pg_get_userbyid(n.nspowner) = current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if owns_schema {
        return Err("application role must not own the fvoci schema".into());
    }

    let owns_protected: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_class c
            INNER JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = 'fvoci'
              AND c.relname IN (
                  'schema_migrations', 'users', 'workspaces', 'memberships',
                  'sessions', 'events', 'audit_log', 'documents', 'document_states',
                  'document_collab_updates', 'document_collab_op_receipts', 'attachments',
                  'projects', 'project_members', 'workflows', 'statuses', 'tasks',
                  'invitations', 'revisions', 'comments', 'api_tokens', 'outbox_consumers',
                  'outbox_failures', 'processed_events', 'attachment_text', 'labels',
                  'task_assignees', 'task_labels', 'milestones', 'task_dependencies',
                  'notifications', 'notification_prefs', 'workspace_holidays', 'ics_tokens',
                  'magic_tokens', 'task_activity', 'webhooks', 'webhook_deliveries',
                  'github_installations', 'github_install_states', 'github_issue_links',
                  'github_deliveries', 'user_mfa', 'identity_links', 'workspace_oidc',
                  'mfa_challenges', 'oidc_states'
              )
              AND pg_get_userbyid(c.relowner) = current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if owns_protected {
        return Err("application role must not own protected fvoci tables".into());
    }

    let inherits_owner: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_namespace n
            CROSS JOIN LATERAL (
                SELECT pg_get_userbyid(n.nspowner) AS owner_name
            ) owner
            WHERE n.nspname = 'fvoci'
              AND pg_has_role(current_user, owner.owner_name, 'MEMBER')
              AND owner.owner_name <> current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if inherits_owner {
        return Err("application role must not inherit the migration owner role".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    // Applied migrations are skipped by version, so editing an accepted file would
    // silently diverge existing installations. Add a new migration instead and
    // append its digest here when it is accepted.
    const ACCEPTED_MIGRATION_SHA256: &[(i32, &str)] = &[
        (
            1,
            "1ed4ce976b341c67fca4932c59f4de151e904697df7ca5ecef3c15d25b2d1680",
        ),
        (
            2,
            "c929fff47207a0fa8d59dd18cb7ad28b36101eab91aba34e744f4e03e9bf1f4e",
        ),
        (
            3,
            "dd8741c56d5e2a2b3ab2fd177c72ee506b82eaa1a0c44cd3ac54c31865e292b0",
        ),
        (
            4,
            "a1a3720d7d57649779093c3eee7fd7f3d2e09e3a9870295c6a5c278e6e19d1ec",
        ),
        (
            5,
            "320f5c989410c0e03f05ec5056e96fb55854b54d8452ccb7f197ef5a7523989b",
        ),
        (
            6,
            "f567d29c7deca648adc0b2b80dda54feae59906bb02453fb62e729df788772d8",
        ),
        (
            7,
            "36752ac87f153e689ba298b5cfab6c9a1375b34b1ec3dc839942d36da8c0b0fa",
        ),
        (
            8,
            "03a94479c2f4e44f2a79e1dfe30f5f3c697eccfb35525b34dacc21f243bb030b",
        ),
        (
            9,
            "640c6063ba24cbe90a00dbd98cf54945752265fbfceef9221ee027bfa26289b6",
        ),
        (
            10,
            "64594b33a13df1ed29479d4b07d692850e0c806b0b76749cc0316e3eaea8dfa8",
        ),
        (
            11,
            "ab17b10baa2533ebccaf994ae5c4d1d0f1ce7bc82a56f9e77371c483f48bf3e5",
        ),
        (
            12,
            "b8964ac88a75c2b5e4264c25de083810971e350550214c61bdd08de6ed695912",
        ),
        (
            13,
            "2ac42b2dc796dddd23d48574135c9e0c50c60cb55c6c3d9960b7faffe9c66708",
        ),
        (
            14,
            "c772193b7b97c1484d6339c69c32a18d8b46644d3cc9700db1b7222157ec0637",
        ),
        (
            15,
            "8b6a424957ef8a5a67804ea8b3f3d9990bbff43c91ab4dad10f2b682e864ce35",
        ),
        (
            16,
            "68dcd7c2f0edd54ee4ba4638dda9edbceef4e58b6174e537fb92c9abfe006ff5",
        ),
        (
            17,
            "f09ba97f766cfa3265c7383b9fd91c6f6fe747581bf7156807f5beffdda84846",
        ),
        (
            18,
            "5999d2f08f5f62f274c445e965bdf938c84b90b52b5557fa60b81f5e29ba002c",
        ),
        (
            19,
            "5da12540460cbda0b823d7e63025ab8e3b371c90cfa1acb09b7b342a8f9b4fc6",
        ),
        (
            20,
            "cd4e1ec73457fed4f759ddbeff9d998a690afde5c3c4f6779ea20b45baf13fb4",
        ),
        (
            21,
            "7a9e03487d30c9e92e4f8b93e69d0bbc1b1ac20771161c6a71ba8ed48a90a50c",
        ),
        (
            22,
            "62ed9fa9bdc77189975c11ddf420a4dccf79ee039f6d2548355f85e6ed6eba3c",
        ),
        (
            23,
            "29b0d5367efb66f104a66794f1c3a3b4a41917e946c7a8077eb316eee700a32c",
        ),
        (
            24,
            "9fcd7f5d12faa7d3108278e95753bca4b60f3f4529b65caf3c94f1f5944a1921",
        ),
        (
            25,
            "265edf3e23543c631c2f515607304e67edf056bab9f5537506739a8e5464d175",
        ),
        (
            26,
            "fc1035798930f9c1c96f654066a8552f03d0700be02199b7f2de9072fc75588d",
        ),
        (
            27,
            "abb60b87ee539663e93c9fa68b8eb986748c2acccd210c0f655c061b378c9d68",
        ),
        (
            28,
            "99627ccc208879f385340ee888f25cfd7e85f2a57b692a9cfc8cfb2280f7216d",
        ),
        (
            29,
            "fa1870fbfc05a77fe6efbe9adaa59f22b275b5faef7fb09712f214374a3df531",
        ),
        (
            30,
            "c5fb3f817fb536b10dd9ab68b626b9cd598c2a4198eded19b2dd8d58c385d1e8",
        ),
        (
            31,
            "c098b60993d173feb3c7f2f4c33a7c4df7a90c6515aa656a1e96400e871ec643",
        ),
        (
            32,
            "dbf49b5d5bf969376406208582f976bd9cc4d04a8b2d76d90fee53d2902b3687",
        ),
        (
            33,
            "4c8310a5380b5e1fa165fb7d3ebc0cdd4237af60ce102551061d37aa4e53fa7b",
        ),
        (
            34,
            "a15a3adb05357c2ac6e8cb70265b7b8319d18c3a934f1ef4658bb0cbe4cee7a4",
        ),
        (
            35,
            "9ab2a7432d458d2663e3caa796cf9ac71a6cda1117a94d1ccdeb5c8bc8585b69",
        ),
        (
            36,
            "6932eba2f27535ac16801d00bafbf5fa1c25e6b0f84c4ff42ea63957103e576c",
        ),
        (
            37,
            "4a67badc60a646dc99a7b7698800cb309a3b1bcae76bf8fb5be7531b58e33b81",
        ),
        (
            38,
            "03d965d84b895263df58bf5520c23ca8e1acc444f082ca20306bcb388a29a487",
        ),
        (
            39,
            "27713b14583fd331f0679244e359ade3ae4a2427d8db06210cbfd4d6dc4f34d5",
        ),
        (
            40,
            "ed2db8ff0ab33ce59fd098d0370e05464779d5197799a16054bf43b7e2b6f33f",
        ),
        (
            41,
            "1d86261505a7065283f7a27b53399e2628602464c576ed36fe7a6a2ff2ff6137",
        ),
        (
            42,
            "b30c81994d86829c682b69c0f849b08575cdb64e17466a39d26f35ae12344431",
        ),
        (
            43,
            "a513c4f1c24e1c0c65c49e78e12131cc82931036c59c377956b5348fe8ce22c8",
        ),
        (
            44,
            "bfabdbe0270e7275212aa45489405d2651526b34108c6d50ff718e243c363d5d",
        ),
        (
            45,
            "34ba8efcdc7d13f9921611aba74a1a9bc13c88c25d8ffcb35282de4919b9f561",
        ),
        (
            46,
            "28a0fcfd7b95711c7bd6e17d49c331a26411046551c0ce3bf6509fb48120a1ad",
        ),
        (
            47,
            "2ec47baddc7570820e2101a8c7dbce72ca7678d84dbf2683e4a463c0b05ac47b",
        ),
        (
            48,
            "b7f943996e9cc9918c90ceb780db285708a31c165368064cd17eb0a5f14ed787",
        ),
        (
            49,
            "ea1cc0783489ce6866a46c6b1363e5bfd89e76f026f27d0ca7615b02c2ece219",
        ),
        (
            50,
            "201e77d18302c25e2b933168d4678a6bb27e251454f1d9d42e5f3aa0ceda09bf",
        ),
        (
            51,
            "aa9315e174e62a101045fd5f254bf98e9f2ca70a08e801e2862d7c0aaaf50008",
        ),
        (
            52,
            "7472a27beda41856d5c93e6a428a0913571696c2727759a6dcf5eabcf78253f3",
        ),
        (
            53,
            "1fac26baa0b5ebaaf97312131dfd10cfb024ad4e516307a95dea5a3653fe5a89",
        ),
        (
            54,
            "53a6ed1058565b61305ed34c41b90a80a2d3f0dd55a4d818813672b1ac43afbc",
        ),
        (
            55,
            "dc60f939985a5c000d57a7f266c12c247373601f92759630b324b22b4a29af45",
        ),
    ];

    #[test]
    fn accepted_migrations_are_immutable_and_ordered() {
        assert_eq!(MIGRATIONS.len(), ACCEPTED_MIGRATION_SHA256.len());
        for ((sql, version), (expected_version, expected)) in
            MIGRATIONS.iter().zip(ACCEPTED_MIGRATION_SHA256)
        {
            assert_eq!(version, expected_version, "migration order changed");
            let digest: String = Sha256::digest(sql.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(
                &digest, expected,
                "accepted migration {version:03} changed; add a new migration instead"
            );
        }
    }

    #[test]
    fn schema_gate_distinguishes_ahead_behind_gap_and_empty() {
        let compiled = compiled_migration_versions();
        let expected = latest_migration_version();
        assert!(schema_gate(&compiled, &compiled).is_ok());

        let mut behind = compiled.clone();
        behind.pop();
        let behind = schema_gate(&behind, &compiled).unwrap_err();
        assert!(
            behind.contains(&format!("behind compiled version {expected}")),
            "{behind}"
        );
        assert!(behind.contains(SCHEMA_GATE_OPERATOR_HINT), "{behind}");

        // A lower version merged after a higher one: max(version) matches but
        // the set does not. The server must still refuse to start.
        let mut gap = compiled.clone();
        let removed = gap.remove(gap.len() - 2);
        let gap = schema_gate(&gap, &compiled).unwrap_err();
        assert!(
            gap.contains(&format!("missing migrations [{removed}]")),
            "{gap}"
        );
        assert!(gap.contains(SCHEMA_GATE_OPERATOR_HINT), "{gap}");

        let mut ahead = compiled.clone();
        ahead.push(expected + 1);
        let ahead = schema_gate(&ahead, &compiled).unwrap_err();
        assert!(
            ahead.contains(&format!("newer than this binary ({expected})")),
            "{ahead}"
        );
        assert!(ahead.contains("deploy a matching fvoci-server"), "{ahead}");
        assert!(!ahead.contains("fvoci-migrate"), "{ahead}");

        let empty = schema_gate(&[], &compiled).unwrap_err();
        assert!(empty.contains("no applied migrations"), "{empty}");
        assert!(empty.contains(SCHEMA_GATE_OPERATOR_HINT), "{empty}");
    }

    #[test]
    fn grant_sql_quotes_role_and_replaces_every_placeholder() {
        let sql = app_role_grant_sql("app\"x");
        assert!(!sql.contains(APP_ROLE_PLACEHOLDER));
        assert!(sql.contains("TO \"app\"\"x\";"));
        assert!(!sql.contains("BEGIN") && !sql.contains("COMMIT"));
    }
}

/// SQLite-family lineage: these are executed steps, never PG version markers.
pub const SQLITE_LINEAGE: &str = "fvoci-sqlite-current-v1";
const SQLITE_MIGRATIONS: &[(&str, &str)] = &[
    (
        include_str!("../../migrations/sqlite/001_current_schema.sql"),
        "0d49ad2c13bcc7942e95441f9965ff3a60f8982b10b3baa55d4b46e00420965f",
    ),
    (
        include_str!("../../migrations/sqlite/002_wiki_create_commands.sql"),
        "8460c39b4f0815fabf9e13e958ab6a99f2415ad10b59e04bd6de397724abe45c",
    ),
    (
        include_str!("../../migrations/sqlite/003_collab_room_fences.sql"),
        "fd9b35411378d7485c40f9fcac2821121eb260c1a09dc7bdeb08a72608ba6d70",
    ),
];

/// Held by the actual server for its entire joined runtime, or exclusively
/// by preparation until every migration connection has closed. Locks use
/// the database inode, so equivalent paths cannot bypass admission.
pub struct SqliteAdmission(std::fs::File);
impl SqliteAdmission {
    pub fn server(path: &std::path::Path) -> Result<Self, sqlx::Error> {
        Self::acquire(path, false)
    }
    fn migration(path: &std::path::Path) -> Result<Self, sqlx::Error> {
        Self::acquire(path, true)
    }
    fn acquire(path: &std::path::Path, migration: bool) -> Result<Self, sqlx::Error> {
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(schema_error(
                "SQLite requires an absolute persistent database file",
            ));
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(migration);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).map_err(sqlx::Error::Io)?;
        let result = if migration {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        result.map_err(|_| {
            schema_error(
                "SQLite server/migrator admission is held; stop live servers before migration",
            )
        })?;
        Ok(Self(file))
    }
}
impl Drop for SqliteAdmission {
    fn drop(&mut self) {
        // Closing the owned OS handle releases this lock, including process
        // termination. Explicit server shutdown joins consumers before here.
        let _ = self.0.unlock();
    }
}

fn schema_error(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Protocol(message.into())
}

/// A migration step's DDL and marker share one reserved writer transaction.
/// Cancellation cannot publish a partial step. Restart checks actual receipts
/// and schema before deciding whether another step is needed.
pub async fn run_sqlite_migrations(path: &std::path::Path) -> Result<(), sqlx::Error> {
    start_sqlite_migration(path)?.wait().await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqliteMigrationDrain {
    Pending,
    Closed,
    Quarantined,
}

struct MigrationOwnerState {
    drain: tokio::sync::watch::Receiver<SqliteMigrationDrain>,
    join: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}
/// A cancellation observer can wait for the finite cleanup owner after the
/// request future is gone. Closed means all owned pool/worker closure finished.
#[derive(Clone)]
pub struct SqliteMigrationObserver(std::sync::Arc<MigrationOwnerState>);
impl SqliteMigrationObserver {
    pub async fn wait(&self) -> Result<SqliteMigrationDrain, sqlx::Error> {
        let mut drain = self.0.drain.clone();
        loop {
            let state = *drain.borrow();
            if state != SqliteMigrationDrain::Pending {
                let join = self
                    .0
                    .join
                    .lock()
                    .map_err(|_| schema_error("migration join lock poisoned"))?
                    .take();
                if let Some(join) = join {
                    join.join()
                        .map_err(|_| schema_error("migration owner thread panicked"))?;
                }
                return Ok(state);
            }
            drain
                .changed()
                .await
                .map_err(|_| schema_error("migration owner ended without a drain receipt"))?;
        }
    }
}

pub struct SqliteMigration {
    cancel: tokio_util::sync::CancellationToken,
    result: Option<tokio::sync::oneshot::Receiver<Result<(), sqlx::Error>>>,
    observer: SqliteMigrationObserver,
}
impl SqliteMigration {
    pub fn observer(&self) -> SqliteMigrationObserver {
        self.observer.clone()
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub async fn wait(mut self) -> Result<(), sqlx::Error> {
        let result = self
            .result
            .take()
            .expect("one migration result receiver")
            .await
            .map_err(|_| schema_error("migration owner ended without a result"))?;
        self.observer.wait().await?;
        result
    }
}
impl Drop for SqliteMigration {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(feature = "db-tests")]
pub struct SqliteMigrationTestControl {
    pub connected: tokio::sync::oneshot::Sender<sqlx::SqlitePool>,
    pub proceed: tokio::sync::oneshot::Receiver<()>,
    pub cleanup_started: Option<tokio::sync::oneshot::Sender<()>>,
}
#[cfg(feature = "db-tests")]
type MigrationControl = Option<SqliteMigrationTestControl>;
#[cfg(not(feature = "db-tests"))]
type MigrationControl = ();

pub fn start_sqlite_migration(path: &std::path::Path) -> Result<SqliteMigration, sqlx::Error> {
    start_sqlite_migration_owned(path, Default::default())
}
#[cfg(feature = "db-tests")]
pub fn start_sqlite_migration_controlled(
    path: &std::path::Path,
    control: SqliteMigrationTestControl,
) -> Result<SqliteMigration, sqlx::Error> {
    start_sqlite_migration_owned(path, Some(control))
}

// Unconfirmed connection initialization or a panicked cleanup cannot authorize
// another owner. Keep that inode quarantined for this process, rather than
// treating SQLx signal-only Drop as shutdown. A CLI restart ends this process.
static QUARANTINED_SQLITE_MIGRATIONS: std::sync::Mutex<Vec<std::sync::Arc<SqliteAdmission>>> =
    std::sync::Mutex::new(Vec::new());
fn quarantine_sqlite_admission(admission: std::sync::Arc<SqliteAdmission>) {
    match QUARANTINED_SQLITE_MIGRATIONS.lock() {
        Ok(mut quarantined) => quarantined.push(admission),
        Err(_) => std::mem::forget(admission), // fail closed even if quarantine bookkeeping poisoned
    }
}
#[derive(Debug, thiserror::Error)]
#[error("SQLite migration cleanup is unconfirmed; admission quarantined until process restart")]
struct MigrationCleanupUnconfirmed(#[source] sqlx::Error);
fn unconfirmed_migration_cleanup(error: sqlx::Error) -> sqlx::Error {
    sqlx::Error::AnyDriverError(Box::new(MigrationCleanupUnconfirmed(error)))
}
fn cleanup_is_unconfirmed(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::AnyDriverError(source) if source.downcast_ref::<MigrationCleanupUnconfirmed>().is_some())
}

// A reference worker without a shutdown receipt remains unconfirmed even
// when rollback of the separate preparation transaction also fails. Ordinary
// validation errors retain the existing rollback-error precedence.
fn sqlite_validation_error_after_rollback(
    primary: sqlx::Error,
    rollback: Result<(), sqlx::Error>,
) -> sqlx::Error {
    if cleanup_is_unconfirmed(&primary) {
        primary
    } else {
        rollback.err().unwrap_or(primary)
    }
}

fn start_sqlite_migration_owned(
    path: &std::path::Path,
    control: MigrationControl,
) -> Result<SqliteMigration, sqlx::Error> {
    let admission = std::sync::Arc::new(SqliteAdmission::migration(path)?);
    let path = path.to_path_buf();
    let cancel = tokio_util::sync::CancellationToken::new();
    let owned_cancel = cancel.clone();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let (drain_tx, drain_rx) = tokio::sync::watch::channel(SqliteMigrationDrain::Pending);
    let state = std::sync::Arc::new(MigrationOwnerState {
        drain: drain_rx,
        join: std::sync::Mutex::new(None),
    });
    // This finite owner has its own runtime: aborting a caller or shutting down
    // its Tokio runtime cannot abort an in-flight COMMIT or skip worker close.
    // No scheduler/daemon or external orchestration process is involved.
    let join = std::thread::Builder::new()
        .name("fvoci-sqlite-migrate".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(sqlx::Error::Io)?;
                runtime.block_on(run_sqlite_migration_owned(&path, &owned_cancel, control))
            }));
            let result = match outcome {
                Ok(result) => result,
                Err(_) => Err(unconfirmed_migration_cleanup(schema_error(
                    "migration execution/cleanup panicked",
                ))),
            };
            let drain = if result.as_ref().err().is_some_and(cleanup_is_unconfirmed) {
                quarantine_sqlite_admission(admission);
                SqliteMigrationDrain::Quarantined
            } else {
                drop(admission);
                SqliteMigrationDrain::Closed
            };
            drain_tx.send_replace(drain);
            let _ = result_tx.send(result);
        })
        .map_err(sqlx::Error::Io)?;
    *state
        .join
        .lock()
        .map_err(|_| schema_error("migration join lock poisoned"))? = Some(join);
    Ok(SqliteMigration {
        cancel,
        result: Some(result_rx),
        observer: SqliteMigrationObserver(state),
    })
}

async fn run_sqlite_migration_owned(
    path: &std::path::Path,
    cancel: &tokio_util::sync::CancellationToken,
    control: MigrationControl,
) -> Result<(), sqlx::Error> {
    let preparation = super::pool::connect_sqlite_prepare(path)
        .await
        .map_err(unconfirmed_migration_cleanup)?;
    let backend = super::backend::Backend::Sqlite(preparation.pool.clone());
    #[cfg(feature = "db-tests")]
    let mut cleanup_started = None;
    #[cfg(feature = "db-tests")]
    if let Some(control) = control {
        let _ = control.connected.send(preparation.pool.clone());
        cleanup_started = control.cleanup_started;
        tokio::select! {
            _ = control.proceed => {},
            _ = cancel.cancelled() => {},
        }
    }
    #[cfg(not(feature = "db-tests"))]
    let _ = control;
    let result = apply_sqlite_migrations(&backend, Some(cancel)).await;
    // Request cancellation does not cancel any operation above or this drain.
    #[cfg(feature = "db-tests")]
    if let Some(started) = cleanup_started {
        let _ = started.send(());
    }
    preparation
        .close_confirmed()
        .await
        .map_err(unconfirmed_migration_cleanup)?;
    result
}

/// Exact old guard-in-request lifecycle, only for the barrier regression.
/// The test pauses a real SQLite COMMIT and observes its premature admission;
/// this is never a production backend/fallback or a new migration policy.
#[cfg(feature = "db-tests")]
pub async fn run_sqlite_migrations_legacy_control(
    path: &std::path::Path,
    control: SqliteMigrationTestControl,
) -> Result<(), sqlx::Error> {
    let _admission = SqliteAdmission::migration(path)?;
    let preparation = super::pool::connect_sqlite_prepare(path).await?;
    let backend = super::backend::Backend::Sqlite(preparation.pool.clone());
    let _ = control.connected.send(preparation.pool.clone());
    let _ = control.proceed.await;
    let result = apply_sqlite_migrations(&backend, None).await;
    backend.close().await?;
    result
}

async fn apply_sqlite_migrations(
    backend: &super::backend::Backend,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(), sqlx::Error> {
    use super::backend::DbTransaction;
    use super::codec::Cell;
    for (index, (sql, digest)) in SQLITE_MIGRATIONS.iter().enumerate() {
        if cancel.is_some_and(|token| token.is_cancelled()) {
            return Err(schema_error(
                "SQLite migration cancelled after in-flight work settled",
            ));
        }
        let mut tx = backend.begin_write().await?;
        let result = async {
            if cancel.is_some_and(|token| token.is_cancelled()) { return Err(schema_error("SQLite migration cancelled before new step")); }
            let DbTransaction::SqliteFamily(family) = &mut tx else {
                return Err(schema_error("SQLite migrations require an actual SQLite-family handle"));
            };
            let applied = sqlite_applied(family).await?;
            verify_sqlite_applied(&applied, false)?;
            verify_sqlite_objects(family, applied.len()).await?;
            if index < applied.len() { return Ok(false); }
            if index != applied.len() { return Err(schema_error("SQLite migration gap")); }
            verify_compiled_sqlite_digest(sql, digest)?;
            if cancel.is_some_and(|token| token.is_cancelled()) { return Err(schema_error("SQLite migration cancelled before DDL")); }
            family.apply_migration_batch(sql).await?;
            if cancel.is_some_and(|token| token.is_cancelled()) { return Err(schema_error("SQLite migration cancelled after DDL; rollback before marker/commit")); }
            family.execute(
                "INSERT INTO schema_migrations(version,lineage,sql_sha256,applied_at) VALUES(?1,?2,?3,unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)",
                &[Cell::Integer((index + 1) as i64), Cell::text(SQLITE_LINEAGE), Cell::text(*digest)],
            ).await?;
            verify_sqlite_objects(family, index + 1).await?;
            Ok(true)
        }.await;
        match result {
            Ok(true) => {
                if cancel.is_some_and(|token| token.is_cancelled()) {
                    tx.rollback().await?;
                    return Err(schema_error("SQLite migration cancelled before commit"));
                }
                tx.commit()
                    .await
                    .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            }
            Ok(false) => tx.rollback().await?,
            Err(error) => {
                return Err(sqlite_validation_error_after_rollback(
                    error,
                    tx.rollback().await,
                ));
            }
        }
    }
    assert_sqlite_schema_current(backend).await.map(|_| ())
}

#[derive(Debug)]
pub struct SqliteCapability {
    pub lineage: &'static str,
    pub applied_steps: usize,
    pub schema_sha256: String,
}

pub async fn assert_sqlite_schema_current(
    backend: &super::backend::Backend,
) -> Result<SqliteCapability, sqlx::Error> {
    use super::backend::DbTransaction;
    let mut tx = backend.begin_read().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            return Err(schema_error(
                "SQLite schema gate requires an actual SQLite-family handle",
            ));
        };
        let applied = sqlite_applied(family).await?;
        verify_sqlite_applied(&applied, true)?;
        let schema_sha256 = verify_sqlite_objects(family, applied.len()).await?;
        Ok(SqliteCapability {
            lineage: SQLITE_LINEAGE,
            applied_steps: applied.len(),
            schema_sha256,
        })
    }
    .await;
    match result {
        Ok(capability) => {
            tx.rollback().await?;
            Ok(capability)
        }
        Err(error) => Err(sqlite_validation_error_after_rollback(
            error,
            tx.rollback().await,
        )),
    }
}

fn verify_compiled_sqlite_digest(sql: &str, digest: &str) -> Result<(), sqlx::Error> {
    use sha2::{Digest, Sha256};
    if hex::encode(Sha256::digest(sql.as_bytes())) != digest {
        return Err(schema_error(
            "compiled SQLite migration digest does not match its registry",
        ));
    }
    Ok(())
}

type SqliteApplied = (i64, String, String);
async fn sqlite_applied(
    family: &mut super::backend::FamilyTx,
) -> Result<Vec<SqliteApplied>, sqlx::Error> {
    let exists = family
        .query(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='schema_migrations'",
            &[],
        )
        .await?;
    if exists[0].cell(0)?.integer()? == 0 {
        return Ok(Vec::new());
    }
    family
        .query(
            "SELECT version,lineage,sql_sha256,applied_at FROM schema_migrations ORDER BY version",
            &[],
        )
        .await?
        .iter()
        .map(|row| {
            // Reject a malformed storage class/time as well as an invalid set.
            row.cell(3)?.datetime()?;
            Ok((
                row.cell(0)?.integer()?,
                row.cell(1)?.string()?,
                row.cell(2)?.string()?,
            ))
        })
        .collect()
}
fn verify_sqlite_applied(applied: &[SqliteApplied], complete: bool) -> Result<(), sqlx::Error> {
    if applied.len() > SQLITE_MIGRATIONS.len()
        || (complete && applied.len() != SQLITE_MIGRATIONS.len())
    {
        return Err(schema_error(
            "SQLite schema is ahead, incomplete or unprepared",
        ));
    }
    for (index, (version, lineage, digest)) in applied.iter().enumerate() {
        let (sql, expected) = SQLITE_MIGRATIONS[index];
        verify_compiled_sqlite_digest(sql, expected)?;
        if *version != (index + 1) as i64 || lineage != SQLITE_LINEAGE || digest != expected {
            return Err(schema_error(
                "SQLite schema has a gap, foreign lineage or changed digest",
            ));
        }
    }
    Ok(())
}

const SQLITE_OBJECTS: &str = "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type COLLATE BINARY,name COLLATE BINARY";
type SqliteObject = (String, String, String, String);
/// Ask the pinned engine to compile the fixed DDL in a separate reference
/// connection. This validates every actual table/index/trigger definition;
/// no handwritten SQL parser, guessed column inventory or marker-only gate.
async fn verify_sqlite_objects(
    family: &mut super::backend::FamilyTx,
    steps: usize,
) -> Result<String, sqlx::Error> {
    use sha2::{Digest, Sha256};
    use sqlx::Connection;
    let mut reference = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true),
    )
    .await
    .map_err(unconfirmed_migration_cleanup)?;
    let expected = async {
        let pin: (String, String) = sqlx::query_as("SELECT sqlite_version(),sqlite_source_id()")
            .fetch_one(&mut reference)
            .await?;
        if pin.0 != super::pool::SQLITE_VERSION || pin.1 != super::pool::SQLITE_SOURCE_ID {
            return Err(schema_error("SQLite schema reference engine pin mismatch"));
        }
        for (sql, digest) in SQLITE_MIGRATIONS.iter().take(steps) {
            verify_compiled_sqlite_digest(sql, digest)?;
            sqlx::raw_sql(sql).execute(&mut reference).await?;
        }
        sqlx::query_as::<_, SqliteObject>(SQLITE_OBJECTS)
            .fetch_all(&mut reference)
            .await
    }
    .await;
    reference
        .close()
        .await
        .map_err(unconfirmed_migration_cleanup)?;
    let expected = expected?;
    let actual = family
        .query(SQLITE_OBJECTS, &[])
        .await?
        .iter()
        .map(|row| {
            Ok((
                row.cell(0)?.string()?,
                row.cell(1)?.string()?,
                row.cell(2)?.string()?,
                row.cell(3)?.string()?,
            ))
        })
        .collect::<Result<Vec<SqliteObject>, sqlx::Error>>()?;
    if actual != expected {
        return Err(schema_error("SQLite schema definitions differ from compiled capability; unmarked/populated or altered schema refused"));
    }
    let bytes = serde_json::to_vec(&(SQLITE_LINEAGE, steps, actual))
        .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod sqlite_rollback_tests {
    use super::*;

    fn unconfirmed_reference() -> sqlx::Error {
        unconfirmed_migration_cleanup(schema_error("reference worker shutdown unconfirmed"))
    }

    #[test]
    fn unconfirmed_primary_survives_failed_rollback() {
        // The original rollback-first `?` loses the typed quarantine reason.
        let legacy_primary = unconfirmed_reference();
        let legacy = Err::<(), _>(schema_error("rollback failed"))
            .err()
            .unwrap_or(legacy_primary);
        assert!(!cleanup_is_unconfirmed(&legacy));
        let fixed = sqlite_validation_error_after_rollback(
            unconfirmed_reference(),
            Err(schema_error("rollback failed")),
        );
        assert!(cleanup_is_unconfirmed(&fixed));
    }

    #[test]
    fn ordinary_primary_retains_failed_rollback_precedence() {
        let fixed = sqlite_validation_error_after_rollback(
            schema_error("schema validation failed"),
            Err(schema_error("rollback failed")),
        );
        assert!(!cleanup_is_unconfirmed(&fixed));
        assert_eq!(
            fixed.to_string(),
            schema_error("rollback failed").to_string()
        );
    }

    #[test]
    fn successful_rollback_retains_primary_classification() {
        let unconfirmed = sqlite_validation_error_after_rollback(unconfirmed_reference(), Ok(()));
        assert!(cleanup_is_unconfirmed(&unconfirmed));
        let ordinary = sqlite_validation_error_after_rollback(
            schema_error("schema validation failed"),
            Ok(()),
        );
        assert!(!cleanup_is_unconfirmed(&ordinary));
        assert_eq!(
            ordinary.to_string(),
            schema_error("schema validation failed").to_string()
        );
    }
}
