//! Allocated test actors and read-only primary witnesses. No HTTP surface,
//! migration/reset/cleanup operation or public SQL executor is exposed.
use super::backend::{Backend, DbTransaction, FamilyTx};
use super::codec::{Cell, FamilyRow};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

fn refused() -> sqlx::Error {
    sqlx::Error::Protocol("allocated Turso UI fixture contract refused".into())
}

#[derive(Debug, thiserror::Error)]
#[error("Turso UI fixture operation failed; finish outcomes retained")]
struct ObservationFailure {
    #[source]
    original: Option<sqlx::Error>,
    rollback_error: Option<sqlx::Error>,
    rollback_attempted: bool,
    commit: &'static str,
}

// Baseline-only context; the original typed error is retained without Display.
#[derive(Debug, thiserror::Error)]
#[error("Turso UI baseline failed at a fixed observation phase")]
struct BaselineFailure {
    phase: &'static str,
    #[source]
    original: sqlx::Error,
    table_comparison: Option<TableComparison>,
}

#[derive(Debug)]
struct TableComparison {
    expected_count: usize,
    actual_count: usize,
    actual_only_count: usize,
    expected_only_count: usize,
    actual_only_underscore_count: usize,
    set_equal: bool,
    order_equal: bool,
    first_mismatch_index: Option<usize>,
    actual_mismatch_expected_index: Option<usize>,
}

impl TableComparison {
    fn identifiers(actual: &[String], expected: &[String]) -> Self {
        // SQLite compares identifiers without ASCII case distinctions:
        // https://sqlite.org/c3ref/stricmp.html and datatype3.html#collation.
        // Catalog BINARY order can change with spelling, so sort after folding.
        // Retain duplicate entries: list equality must refuse extra catalog rows.
        let fold = |names: &[String]| {
            let mut names = names
                .iter()
                .map(|name| name.to_ascii_lowercase())
                .collect::<Vec<_>>();
            names.sort();
            names
        };
        Self::observe(&fold(actual), &fold(expected))
    }

    fn observe(actual: &[String], expected: &[String]) -> Self {
        let actual_set = actual.iter().collect::<BTreeSet<_>>();
        let expected_set = expected.iter().collect::<BTreeSet<_>>();
        let first_mismatch = actual
            .iter()
            .zip(expected)
            .position(|(actual, expected)| actual != expected)
            .or_else(|| {
                (actual.len() != expected.len()).then_some(actual.len().min(expected.len()))
            });
        Self {
            expected_count: expected.len().min(100001),
            actual_count: actual.len().min(100001),
            actual_only_count: actual_set.difference(&expected_set).count().min(100001),
            expected_only_count: expected_set.difference(&actual_set).count().min(100001),
            actual_only_underscore_count: actual_set
                .difference(&expected_set)
                .filter(|name| name.starts_with('_'))
                .count()
                .min(100001),
            set_equal: actual_set == expected_set,
            order_equal: actual == expected,
            first_mismatch_index: first_mismatch.filter(|index| *index <= 100000),
            actual_mismatch_expected_index: first_mismatch
                .and_then(|index| actual.get(index))
                .and_then(|actual| expected.iter().position(|name| name == actual))
                .filter(|index| *index <= 100000),
        }
    }

    fn diagnostic(&self) -> Value {
        json!({
            "expectedCount":self.expected_count,"actualCount":self.actual_count,
            "actualOnlyCount":self.actual_only_count,
            "expectedOnlyCount":self.expected_only_count,
            "actualOnlyUnderscoreCount":self.actual_only_underscore_count,
            "setEqual":self.set_equal,"orderEqual":self.order_equal,
            "firstMismatchIndex":self.first_mismatch_index,
            "actualMismatchExpectedIndex":self.actual_mismatch_expected_index
        })
    }
}

fn baseline_failure(phase: &'static str, original: sqlx::Error) -> sqlx::Error {
    sqlx::Error::AnyDriverError(Box::new(BaselineFailure {
        phase,
        original,
        table_comparison: None,
    }))
}

fn baseline_category(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::AnyDriverError(source) => {
            if let Some(error) = source.downcast_ref::<libsql::Error>() {
                match error {
                    libsql::Error::ConnectionFailed(_) => "request",
                    // Hrana's inner enum is private in the pinned SDK. Do not
                    // parse its Display text to invent a transport/SQL cause.
                    libsql::Error::Hrana(_) => "libsql-hrana",
                    libsql::Error::SqliteFailure(..) | libsql::Error::RemoteSqliteFailure(..) => {
                        "database"
                    }
                    libsql::Error::InvalidColumnType
                    | libsql::Error::InvalidColumnIndex
                    | libsql::Error::ColumnNotFound(_)
                    | libsql::Error::NullValue => "row-conversion",
                    _ => "driver",
                }
            } else {
                "driver"
            }
        }
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => "request",
        sqlx::Error::Database(_) => "database",
        sqlx::Error::Decode(_)
        | sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::ColumnIndexOutOfBounds { .. }
        | sqlx::Error::ColumnNotFound(_)
        | sqlx::Error::RowNotFound => "row-conversion",
        sqlx::Error::Protocol(_) => "protocol",
        sqlx::Error::PoolClosed | sqlx::Error::PoolTimedOut => "pool",
        _ => "other",
    }
}

fn baseline_diagnostic(error: &sqlx::Error) -> Option<Value> {
    let sqlx::Error::AnyDriverError(source) = error else {
        return None;
    };
    let failure = source.downcast_ref::<BaselineFailure>()?;
    let mut diagnostic =
        json!({"phase":failure.phase,"category":baseline_category(&failure.original)});
    if let Some(comparison) = &failure.table_comparison {
        diagnostic["tableComparison"] = comparison.diagnostic();
    }
    Some(diagnostic)
}

fn finished<T>(
    result: Result<T, sqlx::Error>,
    rollback: Result<(), sqlx::Error>,
    commit: &'static str,
) -> Result<T, sqlx::Error> {
    match (result, rollback) {
        (Ok(value), Ok(())) => Ok(value),
        (original, cleanup) => Err(sqlx::Error::AnyDriverError(Box::new(ObservationFailure {
            original: original.err(),
            rollback_error: cleanup.err(),
            rollback_attempted: true,
            commit,
        }))),
    }
}

/// Fixed classifications only. Original typed driver failures stay in memory;
/// no error strings or ownership capabilities enter the diagnostic receipt.
pub fn failure_receipt(error: &sqlx::Error) -> Value {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = source {
        if let Some(failure) = error.downcast_ref::<ObservationFailure>() {
            let mut receipt = json!({"operation": if failure.original.is_some() { "failed" } else { "confirmed" },
                "rollback": if !failure.rollback_attempted { "not-attempted" } else if failure.rollback_error.is_some() { "unknown" } else { "confirmed" }, "commit": failure.commit});
            if let Some(diagnostic) = failure.original.as_ref().and_then(baseline_diagnostic) {
                receipt["baselineFailure"] = diagnostic;
            }
            if let Some(diagnostic) = failure
                .rollback_error
                .as_ref()
                .and_then(baseline_diagnostic)
            {
                receipt["baselineRollbackFailure"] = diagnostic;
            }
            return receipt;
        }
        if error
            .downcast_ref::<super::backend::CommitUnknown>()
            .is_some()
        {
            return json!({"operation":"failed","rollback":"not-attempted","commit":"unknown"});
        }
        if let Some(failure) = error.downcast_ref::<BaselineFailure>() {
            return json!({"operation":"failed","rollback":"not-attempted","commit":"not-attempted",
                "baselineFailure":{"phase":failure.phase,"category":baseline_category(&failure.original)}});
        }
        source = error.source();
    }
    json!({"operation":"failed","rollback":"not-attempted","commit":"not-attempted"})
}

pub fn validate_namespace(namespace: &str) -> Result<(), sqlx::Error> {
    if namespace.len() != 24
        || !namespace.starts_with("tui-")
        || !namespace[4..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(refused());
    }
    Ok(())
}

fn family<'a>(tx: &'a mut DbTransaction<'_>) -> Result<&'a mut FamilyTx, sqlx::Error> {
    match tx {
        DbTransaction::SqliteFamily(f @ FamilyTx::Remote(_)) => Ok(f),
        _ => Err(refused()),
    }
}

fn exact(cell: Cell) -> Value {
    match cell {
        Cell::Null => json!(["null"]),
        Cell::Integer(v) => json!(["integer", v.to_string()]),
        Cell::Text(v) => json!(["text", v]),
        Cell::Blob(v) => json!(["blob", hex::encode(v)]),
    }
}

fn row_hash(row: &FamilyRow) -> Result<String, sqlx::Error> {
    let FamilyRow::Remote(remote) = row else {
        return Err(refused());
    };
    let cells = (0..remote.column_count())
        .map(|i| row.cell(i as usize).map(exact))
        .collect::<Result<Vec<_>, _>>()?;
    let bytes = serde_json::to_vec(&cells).map_err(|_| refused())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(refused());
    }
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn row_allocation(table: &str, row: &FamilyRow) -> Result<Value, sqlx::Error> {
    let FamilyRow::Remote(remote) = row else {
        return Err(refused());
    };
    let mut workspaces = Vec::new();
    let mut actors = Vec::new();
    let mut events = Vec::new();
    let mut own_id = None;
    let mut consumer = None;
    for i in 0..remote.column_count() {
        let name = remote.column_name(i).ok_or_else(refused)?;
        let cell = row.cell(i as usize)?;
        let destination = match name {
            "workspace_id"
            | "personal_workspace_id"
            | "source_workspace_id"
            | "target_workspace_id" => Some(&mut workspaces),
            "user_id" | "actor_user_id" | "owner_user_id" | "created_by" | "invited_by"
            | "uploader_id" => Some(&mut actors),
            "event_id" => Some(&mut events),
            _ => None,
        };
        if let Some(ids) = destination {
            match cell {
                Cell::Null => {}
                Cell::Blob(bytes) if bytes.len() == 16 => ids.push(hex::encode(bytes)),
                _ => return Err(refused()),
            }
        } else if name == "id" && matches!(table, "users" | "workspaces" | "events") {
            let bytes = cell.bytes()?;
            if bytes.len() != 16 {
                return Err(refused());
            }
            own_id = Some(hex::encode(bytes));
        } else if name == "consumer" {
            consumer = Some(cell.string()?);
        }
    }
    Ok(
        json!({"workspaces":workspaces,"actors":actors,"events":events,"self":own_id,"consumer":consumer}),
    )
}

// Private, typed projections for the three audited global counters and their
// concrete allocation witnesses. These are observation records, not SQL input.
const UI_OPERATION_READS: &[(&str, &str)] = &[
    ("event_sequence", "SELECT id,last_seq FROM event_sequence ORDER BY id"),
    ("collab_fence_counter", "SELECT id,next_fence FROM collab_fence_counter ORDER BY id"),
    ("maintenance_job_claims", "SELECT job_key,owner_token,generation,expires_at FROM maintenance_job_claims ORDER BY job_key"),
    ("events", "SELECT seq,workspace_id,actor_user_id FROM events ORDER BY seq LIMIT 10001"),
    ("users", "SELECT id,email,personal_workspace_id FROM users ORDER BY id LIMIT 10001"),
    ("workspaces", "SELECT id,slug,kind FROM workspaces ORDER BY id LIMIT 10001"),
    ("collab_room_fences", "SELECT workspace_id,document_id,owner_token,fence,expires_at FROM collab_room_fences ORDER BY workspace_id,document_id LIMIT 10001"),
    ("task_collab_room_fences", "SELECT workspace_id,task_id,owner_token,fence,expires_at FROM task_collab_room_fences ORDER BY workspace_id,task_id LIMIT 10001"),
    ("outbox_consumers", "SELECT consumer,last_seq,lease_owner,lease_until FROM outbox_consumers ORDER BY consumer LIMIT 10001"),
];

// Fixed current-schema statements, bound to the maintained schema gate below.
// Hashes are preservation witnesses, never a backup or restore format.
const PRESERVATION_READS: &[(&str, &str)] = &[
    ("api_tokens", "SELECT * FROM api_tokens LIMIT 10001"),
    (
        "attachment_object_cleanups",
        "SELECT * FROM attachment_object_cleanups LIMIT 10001",
    ),
    (
        "attachment_text",
        "SELECT * FROM attachment_text LIMIT 10001",
    ),
    ("attachments", "SELECT * FROM attachments LIMIT 10001"),
    ("audit_log", "SELECT * FROM audit_log LIMIT 10001"),
    (
        "body_save_commands",
        "SELECT * FROM body_save_commands LIMIT 10001",
    ),
    (
        "collab_fence_counter",
        "SELECT * FROM collab_fence_counter LIMIT 10001",
    ),
    (
        "collab_room_fences",
        "SELECT * FROM collab_room_fences LIMIT 10001",
    ),
    (
        "collection_choices",
        "SELECT * FROM collection_choices LIMIT 10001",
    ),
    (
        "collection_fields",
        "SELECT * FROM collection_fields LIMIT 10001",
    ),
    (
        "collection_items",
        "SELECT * FROM collection_items LIMIT 10001",
    ),
    (
        "collection_options",
        "SELECT * FROM collection_options LIMIT 10001",
    ),
    (
        "collection_people",
        "SELECT * FROM collection_people LIMIT 10001",
    ),
    (
        "collection_values",
        "SELECT * FROM collection_values LIMIT 10001",
    ),
    (
        "collection_views",
        "SELECT * FROM collection_views LIMIT 10001",
    ),
    ("collections", "SELECT * FROM collections LIMIT 10001"),
    ("comments", "SELECT * FROM comments LIMIT 10001"),
    (
        "document_collab_op_receipts",
        "SELECT * FROM document_collab_op_receipts LIMIT 10001",
    ),
    (
        "document_collab_updates",
        "SELECT * FROM document_collab_updates LIMIT 10001",
    ),
    (
        "document_members",
        "SELECT * FROM document_members LIMIT 10001",
    ),
    (
        "document_states",
        "SELECT * FROM document_states LIMIT 10001",
    ),
    (
        "document_tag_assignments",
        "SELECT * FROM document_tag_assignments LIMIT 10001",
    ),
    ("document_tags", "SELECT * FROM document_tags LIMIT 10001"),
    ("documents", "SELECT * FROM documents LIMIT 10001"),
    ("event_sequence", "SELECT * FROM event_sequence LIMIT 10001"),
    ("events", "SELECT * FROM events LIMIT 10001"),
    (
        "github_deliveries",
        "SELECT * FROM github_deliveries LIMIT 10001",
    ),
    (
        "github_install_states",
        "SELECT * FROM github_install_states LIMIT 10001",
    ),
    (
        "github_installations",
        "SELECT * FROM github_installations LIMIT 10001",
    ),
    (
        "github_issue_links",
        "SELECT * FROM github_issue_links LIMIT 10001",
    ),
    ("group_members", "SELECT * FROM group_members LIMIT 10001"),
    ("groups", "SELECT * FROM groups LIMIT 10001"),
    ("ics_tokens", "SELECT * FROM ics_tokens LIMIT 10001"),
    ("identity_links", "SELECT * FROM identity_links LIMIT 10001"),
    (
        "import_deferred_events",
        "SELECT * FROM import_deferred_events LIMIT 10001",
    ),
    ("import_jobs", "SELECT * FROM import_jobs LIMIT 10001"),
    (
        "instance_config",
        "SELECT * FROM instance_config LIMIT 10001",
    ),
    (
        "instance_settings",
        "SELECT * FROM instance_settings LIMIT 10001",
    ),
    (
        "instance_settings_meta",
        "SELECT * FROM instance_settings_meta LIMIT 10001",
    ),
    ("invitations", "SELECT * FROM invitations LIMIT 10001"),
    ("labels", "SELECT * FROM labels LIMIT 10001"),
    (
        "legal_documents",
        "SELECT * FROM legal_documents LIMIT 10001",
    ),
    ("magic_tokens", "SELECT * FROM magic_tokens LIMIT 10001"),
    (
        "maintenance_job_claims",
        "SELECT * FROM maintenance_job_claims LIMIT 10001",
    ),
    ("memberships", "SELECT * FROM memberships LIMIT 10001"),
    ("mfa_challenges", "SELECT * FROM mfa_challenges LIMIT 10001"),
    ("milestones", "SELECT * FROM milestones LIMIT 10001"),
    (
        "notification_prefs",
        "SELECT * FROM notification_prefs LIMIT 10001",
    ),
    ("notifications", "SELECT * FROM notifications LIMIT 10001"),
    ("oidc_states", "SELECT * FROM oidc_states LIMIT 10001"),
    (
        "outbox_consumers",
        "SELECT * FROM outbox_consumers LIMIT 10001",
    ),
    (
        "outbox_failures",
        "SELECT * FROM outbox_failures LIMIT 10001",
    ),
    (
        "personal_input_commands",
        "SELECT * FROM personal_input_commands LIMIT 10001",
    ),
    (
        "personal_transfer_commands",
        "SELECT * FROM personal_transfer_commands LIMIT 10001",
    ),
    (
        "processed_events",
        "SELECT * FROM processed_events LIMIT 10001",
    ),
    (
        "project_members",
        "SELECT * FROM project_members LIMIT 10001",
    ),
    ("projects", "SELECT * FROM projects LIMIT 10001"),
    (
        "push_deliveries",
        "SELECT * FROM push_deliveries LIMIT 10001",
    ),
    (
        "push_subscriptions",
        "SELECT * FROM push_subscriptions LIMIT 10001",
    ),
    ("revisions", "SELECT * FROM revisions LIMIT 10001"),
    (
        "schema_migrations",
        "SELECT * FROM schema_migrations LIMIT 10001",
    ),
    ("sessions", "SELECT * FROM sessions LIMIT 10001"),
    ("share_links", "SELECT * FROM share_links LIMIT 10001"),
    ("stars", "SELECT * FROM stars LIMIT 10001"),
    ("statuses", "SELECT * FROM statuses LIMIT 10001"),
    ("task_activity", "SELECT * FROM task_activity LIMIT 10001"),
    ("task_assignees", "SELECT * FROM task_assignees LIMIT 10001"),
    (
        "task_collab_op_receipts",
        "SELECT * FROM task_collab_op_receipts LIMIT 10001",
    ),
    (
        "task_collab_room_fences",
        "SELECT * FROM task_collab_room_fences LIMIT 10001",
    ),
    (
        "task_collab_updates",
        "SELECT * FROM task_collab_updates LIMIT 10001",
    ),
    (
        "task_dependencies",
        "SELECT * FROM task_dependencies LIMIT 10001",
    ),
    ("task_labels", "SELECT * FROM task_labels LIMIT 10001"),
    ("task_origins", "SELECT * FROM task_origins LIMIT 10001"),
    ("task_states", "SELECT * FROM task_states LIMIT 10001"),
    (
        "task_timer_audit",
        "SELECT * FROM task_timer_audit LIMIT 10001",
    ),
    (
        "task_timer_commands",
        "SELECT * FROM task_timer_commands LIMIT 10001",
    ),
    (
        "task_timer_legacy_open",
        "SELECT * FROM task_timer_legacy_open LIMIT 10001",
    ),
    (
        "task_timer_runs",
        "SELECT * FROM task_timer_runs LIMIT 10001",
    ),
    (
        "task_timer_segments",
        "SELECT * FROM task_timer_segments LIMIT 10001",
    ),
    ("tasks", "SELECT * FROM tasks LIMIT 10001"),
    ("templates", "SELECT * FROM templates LIMIT 10001"),
    ("time_entries", "SELECT * FROM time_entries LIMIT 10001"),
    ("user_consents", "SELECT * FROM user_consents LIMIT 10001"),
    ("user_mfa", "SELECT * FROM user_mfa LIMIT 10001"),
    ("users", "SELECT * FROM users LIMIT 10001"),
    ("views", "SELECT * FROM views LIMIT 10001"),
    (
        "webhook_deliveries",
        "SELECT * FROM webhook_deliveries LIMIT 10001",
    ),
    ("webhooks", "SELECT * FROM webhooks LIMIT 10001"),
    (
        "wiki_create_commands",
        "SELECT * FROM wiki_create_commands LIMIT 10001",
    ),
    ("workflows", "SELECT * FROM workflows LIMIT 10001"),
    (
        "workspace_holidays",
        "SELECT * FROM workspace_holidays LIMIT 10001",
    ),
    ("workspace_oidc", "SELECT * FROM workspace_oidc LIMIT 10001"),
    ("workspaces", "SELECT * FROM workspaces LIMIT 10001"),
    (
        "zotero_collections",
        "SELECT * FROM zotero_collections LIMIT 10001",
    ),
    (
        "zotero_connectors",
        "SELECT * FROM zotero_connectors LIMIT 10001",
    ),
    (
        "zotero_credentials",
        "SELECT * FROM zotero_credentials LIMIT 10001",
    ),
    ("zotero_links", "SELECT * FROM zotero_links LIMIT 10001"),
    (
        "zotero_memberships",
        "SELECT * FROM zotero_memberships LIMIT 10001",
    ),
    (
        "zotero_references",
        "SELECT * FROM zotero_references LIMIT 10001",
    ),
];

pub async fn capture_baseline(backend: &Backend) -> Result<Value, sqlx::Error> {
    if !matches!(backend, Backend::LibsqlRemote(_)) {
        return Err(baseline_failure("backend-contract", refused()));
    }
    let current = super::migrate::assert_sqlite_schema_current(backend)
        .await
        .map_err(|error| baseline_failure("schema-check", error))?;
    if current.applied_steps != 12 {
        return Err(baseline_failure("schema-contract", refused()));
    }
    let mut tx = backend
        .begin_read()
        .await
        .map_err(|error| baseline_failure("begin-read", error))?;
    let mut phase = "family-contract";
    let mut table_comparison = None;
    let result = async {
        let f = family(&mut tx)?;
        phase = "table-read";
        let tables = f.query("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name", &[]).await?;
        phase = "table-conversion";
        let names = tables.iter().map(|r| r.cell(0)?.string()).collect::<Result<Vec<_>, sqlx::Error>>()?;
        let mut expected = PRESERVATION_READS.iter().map(|(name, _)| name.to_string()).collect::<Vec<_>>();
        expected.sort();
        phase = "table-contract";
        let comparison = TableComparison::identifiers(&names, &expected);
        if !comparison.order_equal {
            table_comparison = Some(comparison);
            return Err(refused());
        }
        let mut fingerprints = BTreeMap::new();
        let mut allocations = BTreeMap::new();
        let mut total = 0usize;
        for (name, sql) in PRESERVATION_READS {
            phase = "preservation-read";
            let rows = f.query(sql, &[]).await?;
            total += rows.len();
            phase = "row-limit";
            if rows.len() > 10000 || total > 100000 { return Err(refused()) }
            let mut hashes = BTreeMap::<String, usize>::new();
            let mut owners = BTreeMap::new();
            for row in &rows {
                phase = "row-hash";
                let hash = row_hash(row)?;
                phase = "row-allocation";
                owners.insert(hash.clone(), row_allocation(name, row)?);
                *hashes.entry(hash).or_default() += 1;
            }
            fingerprints.insert(*name, hashes);
            allocations.insert(*name, owners);
        }
        phase = "summary-read";
        let users = f.query("SELECT count(*) FROM users WHERE deleted_at IS NULL", &[]).await?;
        let mut operations = BTreeMap::new();
        for (name, sql) in UI_OPERATION_READS {
            phase = "ui-read";
            let rows = f.query(sql, &[]).await?;
            phase = "row-limit";
            if rows.len() > 10000 { return Err(refused()) }
            phase = "ui-conversion";
            let cells = rows.iter().map(|row| {
                let FamilyRow::Remote(remote) = row else { return Err(refused()) };
                (0..remote.column_count()).map(|i| row.cell(i as usize).map(exact)).collect::<Result<Vec<_>, _>>()
            }).collect::<Result<Vec<_>, _>>()?;
            operations.insert(*name, cells);
        }
        // Conservative startup admission. Preserve populated ordinary users /
        // workspaces, but do not let normal background work consume old jobs,
        // expired tokens, withdrawn users, deleted workspaces or leased rooms.
        phase = "summary-read";
        let hazards = f.query("SELECT (SELECT count(*) FROM users WHERE deleted_at IS NOT NULL)+(SELECT count(*) FROM workspaces WHERE deleted_at IS NOT NULL)+(SELECT count(*) FROM collab_room_fences WHERE expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000))+(SELECT count(*) FROM task_collab_room_fences WHERE expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000))+(SELECT count(*) FROM maintenance_job_claims WHERE owner_token IS NOT NULL)", &[]).await?;
        let live_outbox = f.query("SELECT count(*) FROM outbox_consumers WHERE lease_owner IS NOT NULL", &[]).await?;
        phase = "ledger-read";
        let ledger = f.query("SELECT version,lineage,sql_sha256,applied_at FROM schema_migrations ORDER BY version", &[]).await?;
        phase = "ledger-conversion";
        let ledger = ledger.iter().map(|row| (0..4).map(|i| row.cell(i).map(exact)).collect::<Result<Vec<_>, _>>()).collect::<Result<Vec<_>, _>>()?;
        phase = "summary-conversion";
        Ok(json!({"schema":1,"backend":"libsql-remote","schemaCurrent":true,
            "schemaSha256":current.schema_sha256,"lineage":current.lineage,
            "ledger":ledger,"setupNeeded":users[0].cell(0)?.integer()? == 0,
            "fingerprints":fingerprints,"allocations":allocations,"rows":total,"operations":operations,
            "startupHazards":hazards[0].cell(0)?.integer()?,"liveOutboxLeases":live_outbox[0].cell(0)?.integer()?}))
    }.await;
    let result = result.map_err(|original| {
        sqlx::Error::AnyDriverError(Box::new(BaselineFailure {
            phase,
            original,
            table_comparison,
        }))
    });
    let rollback = tx
        .rollback()
        .await
        .map_err(|error| baseline_failure("rollback", error));
    finished(result, rollback, "not-attempted")
}

pub struct ActorInput<'a> {
    pub namespace: &'a str,
    pub password_hash: &'a str,
    pub given_name: &'a str,
    pub family_name: &'a str,
}

pub async fn allocate_owner(
    backend: &Backend,
    input: ActorInput<'_>,
) -> Result<Value, sqlx::Error> {
    create_actor(backend, input, true).await
}

pub async fn add_member(backend: &Backend, input: ActorInput<'_>) -> Result<Value, sqlx::Error> {
    create_actor(backend, input, false).await
}

async fn create_actor(
    backend: &Backend,
    input: ActorInput<'_>,
    owner: bool,
) -> Result<Value, sqlx::Error> {
    validate_namespace(input.namespace)?;
    super::migrate::assert_sqlite_schema_current(backend).await?;
    let user = Uuid::now_v7();
    let mut tx = backend.begin_write().await?;
    let result = async {
        let f = family(&mut tx)?;
        // Current populated primary must stay initialized. A blank target is a
        // different execution contract and cannot be silently provisioned here.
        let users = f.query("SELECT count(*) FROM users WHERE deleted_at IS NULL", &[]).await?;
        if users[0].cell(0)?.integer()? == 0 { return Err(refused()) }
        let workspace = f.query("SELECT id FROM workspaces WHERE slug=?1", &[Cell::text(input.namespace)]).await?;
        let workspace = if owner {
            if !workspace.is_empty() { return Err(refused()) }
            Uuid::now_v7()
        } else {
            if workspace.len() != 1 { return Err(refused()) }
            workspace[0].cell(0)?.id()?
        };
        let email = format!("{}-{}@example.invalid", input.namespace, if owner { "owner" } else { "member" });
        if f.execute("INSERT INTO users(id,email,password_hash,given_name,family_name) VALUES(?1,?2,?3,?4,?5)",
            &[Cell::uuid(user),Cell::text(&email),Cell::text(input.password_hash),Cell::text(input.given_name),Cell::text(input.family_name)]).await? != 1 { return Err(refused()) }
        if owner && f.execute("INSERT INTO workspaces(id,slug,name) VALUES(?1,?2,?3)",
            &[Cell::uuid(workspace),Cell::text(input.namespace),Cell::text("Acme 워크스페이스")]).await? != 1 { return Err(refused()) }
        if !owner {
            let owners = f.query("SELECT u.email FROM memberships m JOIN users u ON u.id=m.user_id WHERE m.workspace_id=?1 AND m.role='owner'", &[Cell::uuid(workspace)]).await?;
            if owners.len() != 1 || owners[0].cell(0)?.string()? != format!("{}-owner@example.invalid", input.namespace) { return Err(refused()) }
        }
        f.execute("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)",
            &[Cell::uuid(workspace),Cell::uuid(user),Cell::text(if owner { "owner" } else { "member" })]).await?;
        Ok(json!({"userId":user,"workspaceId":workspace,"namespace":input.namespace,"email":email}))
    }.await;
    let mut value = match result {
        Ok(value) => {
            tx.commit()
                .await
                .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
            value
        }
        Err(error) => {
            return finished(Err(error), tx.rollback().await, "not-attempted");
        }
    };
    // Independent current primary stream; never reconcile an uncertain commit.
    let mut read = backend.begin_read().await.map_err(|error| {
        sqlx::Error::AnyDriverError(Box::new(ObservationFailure {
            original: Some(error),
            rollback_error: None,
            rollback_attempted: false,
            commit: "confirmed",
        }))
    })?;
    let found = async {
        let rows = family(&mut read)?.query("SELECT u.email,m.role FROM users u JOIN memberships m ON m.user_id=u.id WHERE u.id=?1 AND m.workspace_id=?2", &[Cell::uuid(user),Cell::uuid(Uuid::parse_str(value["workspaceId"].as_str().ok_or_else(refused)?).map_err(|_| refused())?)]).await?;
        if rows.len() != 1 || rows[0].cell(0)?.string()? != value["email"].as_str().ok_or_else(refused)? || rows[0].cell(1)?.string()? != if owner { "owner" } else { "member" } { return Err(refused()) }
        Ok(())
    }.await;
    finished(found, read.rollback().await, "confirmed")?;
    value["commit"] = json!("confirmed");
    value["freshPrimaryReadback"] = json!(true);
    Ok(value)
}

pub async fn observe_native(
    backend: &Backend,
    workspace: Uuid,
    ids: &[Uuid],
) -> Result<Value, sqlx::Error> {
    if ids.is_empty() || ids.len() > 100 {
        return Err(refused());
    }
    super::migrate::assert_sqlite_schema_current(backend).await?;
    let mut tx = backend.begin_read().await?;
    let result = async {
        let f = family(&mut tx)?;
        let mut rows = BTreeMap::new();
        for id in ids {
            let args = [Cell::uuid(workspace),Cell::uuid(*id)];
            let state = f.query("SELECT s.state,s.writer_generation,s.snapshot_cutoff_seq,s.tail_seq,d.content_json,d.text,d.version FROM document_states s JOIN documents d ON d.workspace_id=s.workspace_id AND d.id=s.document_id WHERE s.workspace_id=?1 AND s.document_id=?2", &args).await?;
            if state.len() != 1 { return Err(refused()) }
            let s = &state[0];
            let cutoff = s.cell(2)?.integer()?;
            let tail = s.cell(3)?.integer()?;
            let updates = f.query("SELECT seq,op_id,payload FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2 AND seq>?3 AND seq<=?4 ORDER BY seq LIMIT 10001", &[args[0].clone(),args[1].clone(),Cell::Integer(cutoff),Cell::Integer(tail)]).await?;
            let receipts = f.query("SELECT seq,op_id,payload_len,payload_sha256,actor_user_id FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2 ORDER BY seq,op_id LIMIT 10001", &args).await?;
            if updates.len() > 10000 || receipts.len() > 10000 { return Err(refused()) }
            let receipts = receipts.iter().map(|r| Ok(json!({"seq":r.cell(0)?.integer()?,"op":r.cell(1)?.id()?,"len":r.cell(2)?.integer()?,"sha":hex::encode(r.cell(3)?.bytes()?),"actor":r.cell(4)?.id()?}))).collect::<Result<Vec<_>,sqlx::Error>>()?;
            let mut output = Vec::new();
            for (index, u) in updates.iter().enumerate() {
                let seq = u.cell(0)?.integer()?;
                let op = u.cell(1)?.id()?.to_string();
                let bytes = u.cell(2)?.bytes()?;
                let matches = receipts.iter().filter(|r| r["seq"] == seq && r["op"] == op).collect::<Vec<_>>();
                if seq != cutoff + index as i64 + 1 || matches.len() != 1 || matches[0]["len"] != bytes.len() || matches[0]["sha"] != hex::encode(Sha256::digest(&bytes)) { return Err(refused()) }
                output.push(json!({"seq":seq,"op":op,"hex":hex::encode(bytes)}));
            }
            if tail != cutoff + output.len() as i64 { return Err(refused()) }
            let revisions = f.query("SELECT id,y_snapshot,content_json,text,reason,created_at FROM revisions WHERE workspace_id=?1 AND target_id=?2 AND target_kind='document' ORDER BY created_at,id LIMIT 10001", &args).await?;
            if revisions.len() > 10000 { return Err(refused()) }
            let revisions = revisions.iter().map(|r| Ok(json!({"id":r.cell(0)?.id()?,"snapshot":hex::encode(r.cell(1)?.bytes()?),"content":r.cell(2)?.value()?,"text":r.cell(3)?.string()?,"reason":r.cell(4)?.string()?,"createdAtMicros":r.cell(5)?.integer()?.to_string()}))).collect::<Result<Vec<_>,sqlx::Error>>()?;
            let fences = f.query("SELECT workspace_id,document_id,owner_token,fence,expires_at FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2", &args).await?;
            if fences.len() > 1 { return Err(refused()) }
            let fences = fences.iter().map(|r| (0..5).map(|i| r.cell(i).map(exact)).collect::<Result<Vec<_>, _>>()).collect::<Result<Vec<_>, _>>()?;
            rows.insert(id.to_string(),json!({"state":hex::encode(s.cell(0)?.bytes()?),"writer_generation":s.cell(1)?.integer()?,"cutoff":cutoff,"tail":tail,"content":s.cell(4)?.value()?,"text":s.cell(5)?.string()?,"version":s.cell(6)?.integer()?,"updates":output,"receipts":receipts,"revisions":revisions,"roomFences":fences}));
        }
        Ok(json!({"backend":"libsql-remote","workspace_id":workspace,"rows":rows}))
    }.await;
    let rollback = tx.rollback().await;
    finished(result, rollback, "not-attempted")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allocation_namespace_and_exact_storage_witnesses() {
        assert!(validate_namespace("tui-0123456789abcdef0123").is_ok());
        for bad in [
            "acme",
            "tui-0123456789ABCDEF0123",
            "tui-0123456789abcdef0123'",
            "tui-0123456789abcdef012",
        ] {
            assert!(validate_namespace(bad).is_err());
        }
        assert_eq!(
            exact(Cell::Integer(9_007_199_254_740_993)),
            json!(["integer", "9007199254740993"])
        );
        assert_ne!(exact(Cell::Blob(vec![0, 255])), exact(Cell::text("00ff")));
        assert_ne!(exact(Cell::Null), exact(Cell::text("null")));
    }

    #[test]
    fn table_identifiers_fold_ascii_then_sort_without_losing_duplicates() {
        let expected = PRESERVATION_READS
            .iter()
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), 99);
        let mut actual = expected.clone();
        *actual.iter_mut().find(|name| *name == "groups").unwrap() = "GROUPS".into();
        // sqlite_schema ORDER BY name uses BINARY: GROUPS precedes api_tokens.
        // Identifier order must instead be compared after ASCII folding and sorting.
        actual.sort();
        assert_eq!(actual[0], "GROUPS");
        assert!(TableComparison::identifiers(&actual, &expected).order_equal);

        let mut different = actual.clone();
        different[0] = "different_table".into();
        let comparison = TableComparison::identifiers(&different, &expected);
        assert!(!comparison.order_equal);
        assert_eq!(comparison.actual_only_count, 1);
        assert_eq!(comparison.expected_only_count, 1);

        let mut extra = actual.clone();
        extra.push("extra_table".into());
        let comparison = TableComparison::identifiers(&extra, &expected);
        assert!(!comparison.order_equal);
        assert_eq!(comparison.actual_only_count, 1);
        assert_eq!(comparison.expected_only_count, 0);

        let comparison = TableComparison::identifiers(&actual[1..], &expected);
        assert!(!comparison.order_equal);
        assert_eq!(comparison.actual_only_count, 0);
        assert_eq!(comparison.expected_only_count, 1);

        // Compare lists, not deduplicated sets: two spellings of one identifier
        // are still an invalid extra catalog row, even though setEqual is true.
        let mut duplicate = actual.clone();
        duplicate.push("groups".into());
        let comparison = TableComparison::identifiers(&duplicate, &expected);
        assert!(!comparison.order_equal);
        assert!(comparison.set_equal);
        assert_eq!(comparison.actual_count, 100);

        let comparison = TableComparison::identifiers(&["Ä".into()], &["ä".into()]);
        assert!(
            !comparison.order_equal,
            "SQLite identifier folding is ASCII-only"
        );
    }

    #[test]
    fn table_contract_comparison_keeps_indices_without_private_names() {
        let mut expected = PRESERVATION_READS
            .iter()
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>();
        expected.sort();
        let mut reordered = expected.clone();
        reordered.swap(1, 2);
        let reordered = TableComparison::observe(&reordered, &expected).diagnostic();
        assert_eq!(reordered["setEqual"], true);
        assert_eq!(reordered["orderEqual"], false);
        assert_eq!(reordered["firstMismatchIndex"], 1);
        assert_eq!(reordered["actualMismatchExpectedIndex"], 2);
        assert_eq!(reordered["actualOnlyCount"], 0);
        assert_eq!(reordered["expectedOnlyCount"], 0);
        assert_eq!(reordered["actualOnlyUnderscoreCount"], 0);

        let mut unknown = expected.clone();
        unknown[1] = "PRIVATE_TABLE_ENDPOINT_OR_TOKEN".into();
        let comparison = TableComparison::observe(&unknown, &expected);
        let error = sqlx::Error::AnyDriverError(Box::new(BaselineFailure {
            phase: "table-contract",
            original: refused(),
            table_comparison: Some(comparison),
        }));
        let error = finished::<()>(Err(error), Ok(()), "not-attempted").unwrap_err();
        let receipt = failure_receipt(&error);
        assert_eq!(receipt["baselineFailure"]["phase"], "table-contract");
        assert_eq!(receipt["baselineFailure"]["category"], "protocol");
        let comparison = &receipt["baselineFailure"]["tableComparison"];
        assert_eq!(comparison["expectedCount"], expected.len());
        assert_eq!(comparison["actualCount"], expected.len());
        assert_eq!(comparison["setEqual"], false);
        assert_eq!(comparison["firstMismatchIndex"], 1);
        assert_eq!(comparison["actualMismatchExpectedIndex"], Value::Null);
        assert_eq!(comparison["actualOnlyCount"], 1);
        assert_eq!(comparison["expectedOnlyCount"], 1);
        assert_eq!(comparison["actualOnlyUnderscoreCount"], 0);
        assert_eq!(receipt["rollback"], "confirmed");
        assert_eq!(receipt["commit"], "not-attempted");
        assert!(!receipt.to_string().contains("PRIVATE_"));

        unknown[1] = "_PRIVATE_PLATFORM_TABLE".into();
        unknown.sort();
        let underscore = TableComparison::observe(&unknown, &expected).diagnostic();
        assert_eq!(underscore["firstMismatchIndex"], 0);
        assert_eq!(underscore["actualMismatchExpectedIndex"], Value::Null);
        assert_eq!(underscore["actualOnlyCount"], 1);
        assert_eq!(underscore["expectedOnlyCount"], 1);
        assert_eq!(underscore["actualOnlyUnderscoreCount"], 1);
        assert!(!underscore.to_string().contains("PRIVATE_"));

        let mut duplicate = expected.clone();
        duplicate.push(expected[0].clone());
        let comparison = TableComparison::observe(&duplicate, &expected).diagnostic();
        assert_eq!(comparison["setEqual"], true);
        assert_eq!(comparison["actualCount"], expected.len() + 1);
        assert_eq!(comparison["firstMismatchIndex"], expected.len());
        assert_eq!(comparison["actualMismatchExpectedIndex"], 0);
        let missing = TableComparison::observe(&expected[1..], &expected).diagnostic();
        assert_eq!(missing["setEqual"], false);
        assert_eq!(missing["actualMismatchExpectedIndex"], 1);
        let missing_last =
            TableComparison::observe(&expected[..expected.len() - 1], &expected).diagnostic();
        assert_eq!(missing_last["firstMismatchIndex"], expected.len() - 1);
        assert_eq!(missing_last["actualMismatchExpectedIndex"], Value::Null);
        let empty = TableComparison::observe(&[], &expected).diagnostic();
        assert_eq!(empty["actualCount"], 0);
        assert_eq!(empty["firstMismatchIndex"], 0);
        assert_eq!(empty["actualMismatchExpectedIndex"], Value::Null);
    }

    #[test]
    fn baseline_phase_and_typed_category_do_not_publish_error_values() {
        let original = baseline_failure(
            "row-hash",
            sqlx::Error::Protocol("PRIVATE_ROW_OR_ENDPOINT".into()),
        );
        let rollback = baseline_failure(
            "rollback",
            sqlx::Error::AnyDriverError(Box::new(libsql::Error::ConnectionFailed(
                "PRIVATE_AUTH_OR_URL".into(),
            ))),
        );
        let error = finished::<()>(Err(original), Err(rollback), "not-attempted").unwrap_err();
        let receipt = failure_receipt(&error);
        assert_eq!(
            receipt["baselineFailure"],
            json!({"phase":"row-hash","category":"protocol"})
        );
        assert_eq!(
            receipt["baselineRollbackFailure"],
            json!({"phase":"rollback","category":"request"})
        );
        assert_eq!(receipt["operation"], "failed");
        assert_eq!(receipt["rollback"], "unknown");
        assert!(!receipt.to_string().contains("PRIVATE_"));
        let error = baseline_failure("begin-read", sqlx::Error::PoolClosed);
        assert_eq!(
            failure_receipt(&error)["baselineFailure"],
            json!({"phase":"begin-read","category":"pool"})
        );
        assert_eq!(failure_receipt(&error)["rollback"], "not-attempted");
        let error = finished::<()>(
            Ok(()),
            Err(baseline_failure("rollback", refused())),
            "not-attempted",
        )
        .unwrap_err();
        assert!(failure_receipt(&error).get("baselineFailure").is_none());
        assert_eq!(failure_receipt(&error)["operation"], "confirmed");
        assert_eq!(
            failure_receipt(&error)["baselineRollbackFailure"]["phase"],
            "rollback"
        );
    }

    #[test]
    fn observation_retains_original_and_independent_finish_outcomes() {
        let error = finished::<()>(Err(refused()), Err(refused()), "confirmed").unwrap_err();
        assert_eq!(
            failure_receipt(&error),
            json!({"operation":"failed","rollback":"unknown","commit":"confirmed"})
        );
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("missing typed failure")
        };
        let failure = source.downcast_ref::<ObservationFailure>().unwrap();
        assert!(failure.original.is_some());
        assert!(failure.rollback_error.is_some());
        let error = finished::<()>(Err(refused()), Ok(()), "not-attempted").unwrap_err();
        assert_eq!(failure_receipt(&error)["rollback"], "confirmed");
        let error = finished::<()>(Ok(()), Err(refused()), "not-attempted").unwrap_err();
        assert_eq!(failure_receipt(&error)["operation"], "confirmed");
    }
}
