//! Legal documents and consents (source `packages/core/src/{legal,consent}.ts`).

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::admin::{record_instance_change, require_live_instance_admin, InstanceChange};
use crate::db::context::{clear_self_user, set_self_user, set_system, set_tenant};
use crate::db::identity::lock_sign_in;
use crate::db::workspace::{membership_role, WorkspaceRole};

#[derive(Debug, Clone)]
pub struct LegalDocument {
    pub id: Uuid,
    pub kind: String,
    pub version: i32,
    pub title: String,
    pub body_markdown: String,
    pub body_html: String,
    pub required: bool,
    pub effective_at: DateTime<Utc>,
    pub published_at: DateTime<Utc>,
}

type LegalScan = (
    Uuid,
    String,
    i32,
    String,
    String,
    String,
    bool,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn from_scan(row: LegalScan) -> LegalDocument {
    let (id, kind, version, title, body_markdown, body_html, required, effective_at, published_at) =
        row;
    LegalDocument {
        id,
        kind,
        version,
        title,
        body_markdown,
        body_html,
        required,
        effective_at,
        published_at,
    }
}

const LEGAL_COLUMNS: &str =
    "id, kind, version, title, body_markdown, body_html, required, effective_at, published_at";

/// Kind shape shared by the route parameter and the publish body.
pub fn is_legal_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= 50
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub async fn latest_legal(pool: &PgPool, kind: &str) -> Result<Option<LegalDocument>, sqlx::Error> {
    let row: Option<LegalScan> = sqlx::query_as(&format!(
        "SELECT {LEGAL_COLUMNS} FROM fvoci.legal_documents WHERE kind = $1 ORDER BY version DESC LIMIT 1"
    ))
    .bind(kind)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(from_scan))
}

pub async fn legal_version(
    pool: &PgPool,
    kind: &str,
    version: i32,
) -> Result<Option<LegalDocument>, sqlx::Error> {
    let row: Option<LegalScan> = sqlx::query_as(&format!(
        "SELECT {LEGAL_COLUMNS} FROM fvoci.legal_documents WHERE kind = $1 AND version = $2"
    ))
    .bind(kind)
    .bind(version)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(from_scan))
}

pub async fn list_legal_versions(
    pool: &PgPool,
    kind: &str,
) -> Result<Vec<LegalDocument>, sqlx::Error> {
    let rows: Vec<LegalScan> = sqlx::query_as(&format!(
        "SELECT {LEGAL_COLUMNS} FROM fvoci.legal_documents WHERE kind = $1 ORDER BY version DESC"
    ))
    .bind(kind)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(from_scan).collect())
}

/// Source `listRequiredLatest`: the latest version of each kind, kept when
/// that latest version is required; ordered by kind descending.
const REQUIRED_LATEST_SQL: &str = r#"
    SELECT id, kind, version, title, body_markdown, body_html, required, effective_at,
           published_at
    FROM (
        SELECT DISTINCT ON (kind) id, kind, version, title, body_markdown, body_html, required,
               effective_at, published_at
        FROM fvoci.legal_documents
        ORDER BY kind, version DESC
    ) latest
    WHERE required
    ORDER BY kind DESC
"#;

pub async fn required_latest<'e, E>(executor: E) -> Result<Vec<LegalDocument>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let rows: Vec<LegalScan> = sqlx::query_as(REQUIRED_LATEST_SQL)
        .fetch_all(executor)
        .await?;
    Ok(rows.into_iter().map(from_scan).collect())
}

/// Source `hasRequiredConsentCoverage`: every required latest document is
/// named with its exact version.
pub fn covers_required(required: &[LegalDocument], items: &[(String, i32)]) -> bool {
    required.iter().all(|doc| {
        items
            .iter()
            .rev()
            .find(|(kind, _)| *kind == doc.kind)
            .is_some_and(|(_, version)| *version == doc.version)
    })
}

pub struct LegalPublishInput {
    pub kind: String,
    pub title: String,
    pub body_markdown: String,
    pub body_html: String,
    pub required: bool,
    pub effective_at: DateTime<Utc>,
}

/// Source `publishLegalDocument`: the next version of `kind`, with the admin
/// check under the actor's sign-in row lock and the `legal.published` event
/// and audit rows in the same transaction.
pub async fn publish_legal(
    pool: &PgPool,
    actor: Uuid,
    input: LegalPublishInput,
    ip: Option<&str>,
) -> Result<Option<LegalDocument>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, actor).await?;
    if !require_live_instance_admin(&mut tx, actor).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    set_system(&mut tx).await?;
    // Serialize publishers of one kind so two versions never race for N+1.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('fvoci.legal:' || $1))")
        .bind(&input.kind)
        .execute(&mut *tx)
        .await?;
    let latest: Option<i32> =
        sqlx::query_scalar("SELECT max(version) FROM fvoci.legal_documents WHERE kind = $1")
            .bind(&input.kind)
            .fetch_one(&mut *tx)
            .await?;
    let version = latest.unwrap_or(0) + 1;
    let row: LegalScan = sqlx::query_as(&format!(
        r#"
        INSERT INTO fvoci.legal_documents (
            id, kind, version, title, body_markdown, body_html, required, effective_at,
            published_at, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), $9)
        RETURNING {LEGAL_COLUMNS}
        "#
    ))
    .bind(Uuid::now_v7())
    .bind(&input.kind)
    .bind(version)
    .bind(&input.title)
    .bind(&input.body_markdown)
    .bind(&input.body_html)
    .bind(input.required)
    .bind(input.effective_at)
    .bind(actor)
    .fetch_one(&mut *tx)
    .await?;
    let doc = from_scan(row);
    record_instance_change(
        &mut tx,
        InstanceChange {
            actor_user_id: actor,
            verb: "legal.published",
            target: Some(("legal_document", doc.id)),
            payload: json!({ "kind": doc.kind, "version": doc.version, "required": doc.required }),
            ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Some(doc))
}

async fn consented_pairs(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Vec<(String, i32)>, sqlx::Error> {
    sqlx::query_as("SELECT kind, version FROM fvoci.user_consents WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(&mut **tx)
        .await
}

async fn pending_in(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Vec<LegalDocument>, sqlx::Error> {
    let required = required_latest(&mut **tx).await?;
    let consented = consented_pairs(tx, user_id).await?;
    Ok(required
        .into_iter()
        .filter(|doc| {
            !consented
                .iter()
                .any(|(kind, version)| *kind == doc.kind && *version == doc.version)
        })
        .collect())
}

/// Source `pendingConsents`.
pub async fn pending_consents(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<LegalDocument>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_self_user(&mut tx, user_id).await?;
    let pending = pending_in(&mut tx, user_id).await?;
    clear_self_user(&mut tx).await?;
    tx.commit().await?;
    Ok(pending)
}

/// The 428 gate: one definer call per cookie request.
pub async fn session_consent_pending(pool: &PgPool, token_hash: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT fvoci.app_session_consent_pending($1)")
        .bind(token_hash)
        .fetch_one(pool)
        .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentChannel {
    Signup,
    Gate,
}

impl ConsentChannel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Signup => "signup",
            Self::Gate => "gate",
        }
    }
}

/// Inserts consent rows for the items naming the current latest version of
/// their kind (stale or unknown items are skipped, like the source). Call with
/// `app.self_user_id` set to `user_id` or in the system context.
pub(crate) async fn insert_consents(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    items: &[(String, i32)],
    ip: Option<&str>,
    channel: ConsentChannel,
) -> Result<(), sqlx::Error> {
    for (kind, version) in items {
        sqlx::query(
            r#"
            INSERT INTO fvoci.user_consents (id, user_id, kind, version, consented_at, ip, channel)
            SELECT $1, $2, $3, $4, now(), $5::inet, $6
            WHERE $4 = (SELECT max(version) FROM fvoci.legal_documents WHERE kind = $3)
            ON CONFLICT (user_id, kind, version) DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(kind)
        .bind(*version)
        .bind(ip)
        .bind(channel.as_str())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum RecordConsentsOutcome {
    Recorded,
    NotLive,
}

/// Source `recordConsents` (channel `gate`): under the user's sign-in row lock
/// the user must still be live; when nothing remains pending the
/// `consent.recorded` event and audit rows commit with the consents.
pub async fn record_consents(
    pool: &PgPool,
    user_id: Uuid,
    items: &[(String, i32)],
    ip: Option<&str>,
) -> Result<RecordConsentsOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, user_id).await?;
    let live: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL AND suspended_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if live.is_none() {
        tx.rollback().await?;
        return Ok(RecordConsentsOutcome::NotLive);
    }
    set_self_user(&mut tx, user_id).await?;
    insert_consents(&mut tx, user_id, items, ip, ConsentChannel::Gate).await?;
    if pending_in(&mut tx, user_id).await?.is_empty() {
        record_instance_change(
            &mut tx,
            InstanceChange {
                actor_user_id: user_id,
                verb: "consent.recorded",
                target: Some(("user", user_id)),
                payload: json!({ "userId": user_id.to_string(), "channel": "gate" }),
                ip,
            },
        )
        .await?;
    }
    clear_self_user(&mut tx).await?;
    tx.commit().await?;
    Ok(RecordConsentsOutcome::Recorded)
}

#[derive(Debug, Clone)]
pub struct MemberConsents {
    pub user_id: Uuid,
    pub consents: Vec<(String, i32, DateTime<Utc>)>,
}

/// Source `listWorkspaceConsents`: workspace admins (or owners) see the
/// consents of every member. `None` = not found / not allowed (404).
pub async fn workspace_consents(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Vec<MemberConsents>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let live: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL FOR SHARE",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let role = membership_role(&mut tx, workspace_id, user_id).await?;
    let allowed = live.is_some()
        && matches!(
            role,
            Some(WorkspaceRole::Owner) | Some(WorkspaceRole::Admin)
        );
    if !allowed {
        tx.rollback().await?;
        return Ok(None);
    }
    let rows: Vec<(Uuid, String, i32, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT c.user_id, c.kind, c.version, c.consented_at
        FROM fvoci.memberships m
        INNER JOIN fvoci.user_consents c ON c.user_id = m.user_id
        WHERE m.workspace_id = $1
        ORDER BY c.user_id, c.consented_at, c.kind
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut members: Vec<MemberConsents> = Vec::new();
    for (uid, kind, version, at) in rows {
        match members.last_mut() {
            Some(last) if last.user_id == uid => last.consents.push((kind, version, at)),
            _ => members.push(MemberConsents {
                user_id: uid,
                consents: vec![(kind, version, at)],
            }),
        }
    }
    Ok(Some(members))
}
