use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use crate::db::context::set_self_user;
use crate::display_id::format_display_id;
use crate::error::{AppError, ProblemCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationFilter {
    All,
    Unread,
    Archived,
}

impl NotificationFilter {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "unread" => Some(Self::Unread),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Unread => "unread",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Document,
    Task,
}

#[derive(Debug, Clone)]
pub struct NotificationRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub event_id: Uuid,
    pub verb: String,
    pub actor_user_id: Option<Uuid>,
    pub actor_given_name: Option<String>,
    pub actor_family_name: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub display_id: Option<String>,
    pub payload: Value,
    pub read_at: Option<DateTime<Utc>>,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NotificationPage {
    pub items: Vec<NotificationRow>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct NotificationPrefs {
    pub in_app: bool,
    pub mail_immediate: bool,
    pub mail_digest: bool,
}

pub const DEFAULT_PREFS: NotificationPrefs = NotificationPrefs {
    in_app: true,
    mail_immediate: true,
    mail_digest: false,
};

#[derive(Debug)]
pub enum NotificationDbError {
    NotFound,
    Forbidden,
    InvalidCursor,
    InvalidInput,
}

impl From<NotificationDbError> for AppError {
    fn from(value: NotificationDbError) -> Self {
        match value {
            NotificationDbError::InvalidCursor => AppError {
                status: axum::http::StatusCode::BAD_REQUEST,
                code: ProblemCode::InvalidInput,
                source: None,
                params: Some(serde_json::json!({"code":"invalid_cursor"})),
                retry_after: None,
            },
            NotificationDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput),
            NotificationDbError::NotFound | NotificationDbError::Forbidden => {
                AppError::from_code(ProblemCode::NotFound)
            }
        }
    }
}

pub struct NotificationInsert {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub event_id: Uuid,
    pub verb: String,
    pub actor_user_id: Option<Uuid>,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub payload: Value,
}

pub async fn insert_many(
    tx: &mut Transaction<'_, Postgres>,
    rows: &[NotificationInsert],
) -> Result<(), sqlx::Error> {
    for row in rows {
        sqlx::query(
            r#"
            INSERT INTO fvoci.notifications (
                id, workspace_id, user_id, event_id, verb, actor_user_id,
                target_type, target_id, payload
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (workspace_id, user_id, event_id) DO NOTHING
            "#,
        )
        .bind(row.id)
        .bind(row.workspace_id)
        .bind(row.user_id)
        .bind(row.event_id)
        .bind(&row.verb)
        .bind(row.actor_user_id)
        .bind(&row.target_type)
        .bind(row.target_id)
        .bind(&row.payload)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn find_prefs_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<NotificationPrefs>, sqlx::Error> {
    let row: Option<(bool, bool, bool)> = sqlx::query_as(
        r#"
        SELECT in_app, mail_immediate, mail_digest
        FROM fvoci.notification_prefs
        WHERE workspace_id = $1 AND user_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(
        row.map(|(in_app, mail_immediate, mail_digest)| NotificationPrefs {
            in_app,
            mail_immediate,
            mail_digest,
        }),
    )
}

pub fn resolved_store_prefs(row: Option<NotificationPrefs>) -> NotificationPrefs {
    match row {
        Some(row) => NotificationPrefs {
            in_app: row.in_app,
            mail_immediate: row.mail_immediate,
            mail_digest: row.mail_digest,
        },
        None => DEFAULT_PREFS,
    }
}

fn kind_sql(kinds: Option<&[ContentKind]>) -> String {
    match kinds {
        None => "TRUE".to_string(),
        Some(kinds) => {
            let document = kinds.contains(&ContentKind::Document);
            let task = kinds.contains(&ContentKind::Task);
            format!(
                "(({document} AND (n.target_type = 'document' OR (n.target_type = 'comment' AND n.payload->>'documentId' IS NOT NULL))) \
                  OR ({task} AND (n.target_type = 'task' OR (n.target_type = 'comment' AND n.payload->>'taskId' IS NOT NULL))))"
            )
        }
    }
}

fn filter_sql(filter: NotificationFilter) -> &'static str {
    match filter {
        NotificationFilter::All => "n.archived_at IS NULL",
        NotificationFilter::Unread => "n.read_at IS NULL AND n.archived_at IS NULL",
        NotificationFilter::Archived => "n.archived_at IS NOT NULL",
    }
}

#[derive(Debug, Clone)]
struct ListCursor {
    ca: DateTime<Utc>,
    id: Uuid,
}

fn encode_cursor(ca: DateTime<Utc>, id: Uuid, filter: NotificationFilter) -> String {
    use base64::Engine;
    let payload = serde_json::json!({
        "ca": ca.to_rfc3339_opts(SecondsFormat::Millis, true),
        "id": id.to_string(),
        "f": { "filter": filter.as_str() },
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

fn page_order(left: &NotificationRow, right: &NotificationRow) -> std::cmp::Ordering {
    right
        .created_at
        .timestamp_millis()
        .cmp(&left.created_at.timestamp_millis())
        .then_with(|| right.id.cmp(&left.id))
}

fn decode_cursor(raw: &str, filter: NotificationFilter) -> Result<ListCursor, NotificationDbError> {
    use base64::Engine;
    if raw.len() > 1024 {
        return Err(NotificationDbError::InvalidCursor);
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| NotificationDbError::InvalidCursor)?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| NotificationDbError::InvalidCursor)?;
    let object = value
        .as_object()
        .ok_or(NotificationDbError::InvalidCursor)?;
    if object
        .keys()
        .any(|key| key != "ca" && key != "id" && key != "f")
    {
        return Err(NotificationDbError::InvalidCursor);
    }
    let ca = object
        .get("ca")
        .and_then(Value::as_str)
        .ok_or(NotificationDbError::InvalidCursor)?;
    let ca = DateTime::parse_from_rfc3339(ca)
        .map_err(|_| NotificationDbError::InvalidCursor)?
        .with_timezone(&Utc);
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or(NotificationDbError::InvalidCursor)?;
    let id = Uuid::parse_str(id).map_err(|_| NotificationDbError::InvalidCursor)?;
    let fingerprint = object
        .get("f")
        .and_then(Value::as_object)
        .ok_or(NotificationDbError::InvalidCursor)?;
    let cursor_filter = fingerprint
        .get("filter")
        .and_then(Value::as_str)
        .ok_or(NotificationDbError::InvalidCursor)?;
    if cursor_filter != filter.as_str() {
        return Err(NotificationDbError::InvalidCursor);
    }
    Ok(ListCursor { ca, id })
}

async fn require_member(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    user: Uuid,
    credential: Uuid,
) -> Result<Result<(), NotificationDbError>, sqlx::Error> {
    require_notification_member(
        &mut OperationTx::Postgres(tx),
        workspace,
        user,
        credential,
        false,
    )
    .await
}
async fn require_notification_member(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    user: Uuid,
    credential: Uuid,
    write: bool,
) -> Result<Result<(), NotificationDbError>, sqlx::Error> {
    op.set_tenant(workspace).await?;
    if let OperationTx::Postgres(tx) = op {
        set_self_user(tx, user).await?;
    }
    if write {
        op.lock_membership_users(&[user]).await?;
    }
    let live = if write {
        op.recheck_session(user, credential).await?
    } else {
        op.session_is_live(user, credential).await?
    };
    if !live {
        return Ok(Err(NotificationDbError::Forbidden));
    }
    if !op.workspace_is_live(workspace).await?
        || op.membership_role(workspace, user, write).await?.is_none()
    {
        return Ok(Err(NotificationDbError::NotFound));
    }
    Ok(Ok(()))
}

pub struct ListNotificationsQuery<'a> {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub filter: NotificationFilter,
    pub cursor: Option<&'a str>,
    pub limit: i32,
    pub allowed_kinds: Option<&'a [ContentKind]>,
}

pub async fn list_notifications(
    pool: &PgPool,
    query: ListNotificationsQuery<'_>,
) -> Result<Result<NotificationPage, NotificationDbError>, sqlx::Error> {
    let ListNotificationsQuery {
        workspace_id,
        user_id,
        session_id,
        filter,
        cursor,
        limit,
        allowed_kinds,
    } = query;
    let decoded = match cursor {
        None => None,
        Some(raw) => match decode_cursor(raw, filter) {
            Ok(cursor) => Some(cursor),
            Err(err) => return Ok(Err(err)),
        },
    };
    let mut tx = pool.begin().await?;
    if let Err(err) = require_member(&mut tx, workspace_id, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let prefs = resolved_store_prefs(find_prefs_tx(&mut tx, workspace_id, user_id).await?);
    if !prefs.in_app {
        tx.commit().await?;
        return Ok(Ok(NotificationPage {
            items: Vec::new(),
            next_cursor: None,
        }));
    }
    let page = list_page(
        &mut tx,
        workspace_id,
        user_id,
        filter,
        decoded.as_ref(),
        limit,
        allowed_kinds,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(page))
}

async fn list_page(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    filter: NotificationFilter,
    cursor: Option<&ListCursor>,
    limit: i32,
    allowed_kinds: Option<&[ContentKind]>,
) -> Result<NotificationPage, sqlx::Error> {
    let kind = kind_sql(allowed_kinds);
    let filter_clause = filter_sql(filter);
    let sql = format!(
        r#"
        SELECT
            n.id, n.workspace_id, n.event_id, n.verb, n.actor_user_id,
            n.target_type, n.target_id, n.payload, n.read_at, n.archived_at, n.created_at
        FROM fvoci.notifications n
        WHERE n.workspace_id = $1
          AND n.user_id = $2
          AND {kind}
          AND {filter_clause}
          AND (
              $3::timestamptz IS NULL
              OR (date_trunc('milliseconds', n.created_at), n.id)
                 < (date_trunc('milliseconds', $3::timestamptz), $4::uuid)
          )
        ORDER BY date_trunc('milliseconds', n.created_at) DESC, n.id DESC
        LIMIT $5
        "#
    );
    let fetch_limit = (limit as i64) + 1;
    let rows = sqlx::query(&sql)
        .bind(workspace_id)
        .bind(user_id)
        .bind(cursor.map(|c| c.ca))
        .bind(cursor.map(|c| c.id))
        .bind(fetch_limit)
        .fetch_all(&mut **tx)
        .await?;
    let has_more = rows.len() as i64 > limit as i64;
    let page_rows: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let mut items = Vec::with_capacity(page_rows.len());
    for row in page_rows {
        items.push(map_list_row(tx, workspace_id, row).await?);
    }
    let next_cursor = if has_more {
        items
            .last()
            .map(|item| encode_cursor(item.created_at, item.id, filter))
    } else {
        None
    };
    Ok(NotificationPage { items, next_cursor })
}

async fn map_list_row(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    row: sqlx::postgres::PgRow,
) -> Result<NotificationRow, sqlx::Error> {
    let actor_user_id: Option<Uuid> = row.get("actor_user_id");
    let (actor_given_name, actor_family_name) = actor_names(tx, actor_user_id).await?;
    let target_type: Option<String> = row.get("target_type");
    let target_id: Option<Uuid> = row.get("target_id");
    let payload: Value = row.get("payload");
    let display_id = display_id_for(
        tx,
        workspace_id,
        target_type.as_deref(),
        target_id,
        &payload,
    )
    .await?;
    Ok(NotificationRow {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        event_id: row.get("event_id"),
        verb: row.get("verb"),
        actor_user_id,
        actor_given_name,
        actor_family_name,
        target_type,
        target_id,
        display_id,
        payload,
        read_at: row.get("read_at"),
        archived_at: row.get("archived_at"),
        created_at: row.get("created_at"),
    })
}

async fn actor_names(
    tx: &mut Transaction<'_, Postgres>,
    actor_user_id: Option<Uuid>,
) -> Result<(Option<String>, Option<String>), sqlx::Error> {
    let Some(actor_user_id) = actor_user_id else {
        return Ok((None, None));
    };
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT given_name, family_name
        FROM fvoci.users
        WHERE id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match row {
        Some((given, family)) => (Some(given), family),
        None => (None, None),
    })
}

fn payload_uuid(payload: &Value, key: &str) -> Option<Uuid> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

pub(crate) async fn display_id_for(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target_type: Option<&str>,
    target_id: Option<Uuid>,
    payload: &Value,
) -> Result<Option<String>, sqlx::Error> {
    match display_target(target_type, target_id, payload) {
        Some(("task", id)) => task_display_id(tx, workspace_id, id).await,
        Some(("document", id)) => document_display_id(tx, workspace_id, id).await,
        Some(("project", id)) => project_display_id(tx, workspace_id, id).await,
        _ => Ok(None),
    }
}

fn display_target(
    target_type: Option<&str>,
    target_id: Option<Uuid>,
    payload: &Value,
) -> Option<(&'static str, Uuid)> {
    if let Some(id) = target_id {
        match target_type {
            Some("task") => return Some(("task", id)),
            Some("document") => return Some(("document", id)),
            Some("project") => return Some(("project", id)),
            _ => {}
        }
    }
    if target_type == Some("comment") {
        if let Some(id) = payload_uuid(payload, "taskId") {
            return Some(("task", id));
        }
        if let Some(id) = payload_uuid(payload, "documentId") {
            return Some(("document", id));
        }
    }
    None
}

async fn task_display_id(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String, i32)> = sqlx::query_as(
        r#"
        SELECT p.key, t.number
        FROM fvoci.tasks t
        INNER JOIN fvoci.projects p
            ON p.workspace_id = t.workspace_id AND p.id = t.project_id
        WHERE t.workspace_id = $1 AND t.id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(key, number)| format_display_id(&key, number)))
}

async fn document_display_id(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<Uuid>, i32)> = sqlx::query_as(
        r#"
        SELECT project_id, number
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((project_id, number)) = row else {
        return Ok(None);
    };
    if let Some(project_id) = project_id {
        let key: Option<(String,)> =
            sqlx::query_as("SELECT key FROM fvoci.projects WHERE workspace_id = $1 AND id = $2")
                .bind(workspace_id)
                .bind(project_id)
                .fetch_optional(&mut **tx)
                .await?;
        return Ok(key.map(|(key,)| format_display_id(&key, number)));
    }
    Ok(Some(format_display_id("WIKI", number)))
}

async fn project_display_id(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT key FROM fvoci.projects WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(project_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(key,)| key))
}

pub async fn unread_count(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    allowed_kinds: Option<&[ContentKind]>,
) -> Result<Result<i64, NotificationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_member(&mut tx, workspace_id, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let prefs = resolved_store_prefs(find_prefs_tx(&mut tx, workspace_id, user_id).await?);
    if !prefs.in_app {
        tx.commit().await?;
        return Ok(Ok(0));
    }
    let kind = kind_sql(allowed_kinds);
    let sql = format!(
        r#"
        SELECT count(*)
        FROM fvoci.notifications n
        WHERE n.workspace_id = $1
          AND n.user_id = $2
          AND {kind}
          AND n.read_at IS NULL
          AND n.archived_at IS NULL
        "#
    );
    let count: (i64,) = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(count.0))
}

pub struct SetNotificationFlags<'a> {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub notification_id: Uuid,
    pub read: Option<bool>,
    pub archived: Option<bool>,
    pub allowed_kinds: Option<&'a [ContentKind]>,
}

pub async fn set_flags(
    pool: &PgPool,
    input: SetNotificationFlags<'_>,
) -> Result<Result<bool, NotificationDbError>, sqlx::Error> {
    let SetNotificationFlags {
        workspace_id,
        user_id,
        session_id,
        notification_id,
        read,
        archived,
        allowed_kinds,
    } = input;
    if read.is_none() && archived.is_none() {
        return Ok(Err(NotificationDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    if let Err(err) = require_notification_member(
        &mut OperationTx::Postgres(&mut tx),
        workspace_id,
        user_id,
        session_id,
        true,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let kind = kind_sql(allowed_kinds);
    let sql = format!(
        r#"
        UPDATE fvoci.notifications n
        SET
            read_at = CASE WHEN $4::boolean IS NULL THEN n.read_at
                           WHEN $4 THEN now()
                           ELSE NULL END,
            archived_at = CASE WHEN $5::boolean IS NULL THEN n.archived_at
                               WHEN $5 THEN now()
                               ELSE NULL END
        WHERE n.workspace_id = $1
          AND n.user_id = $2
          AND n.id = $3
          AND {kind}
        RETURNING n.id
        "#
    );
    let updated: Option<(Uuid,)> = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(user_id)
        .bind(notification_id)
        .bind(read)
        .bind(archived)
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(updated.is_some()))
}

pub async fn read_all(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    allowed_kinds: Option<&[ContentKind]>,
) -> Result<Result<i64, NotificationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_notification_member(
        &mut OperationTx::Postgres(&mut tx),
        workspace_id,
        user_id,
        session_id,
        true,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let kind = kind_sql(allowed_kinds);
    let sql = format!(
        r#"
        UPDATE fvoci.notifications n
        SET read_at = now()
        WHERE n.workspace_id = $1
          AND n.user_id = $2
          AND {kind}
          AND n.read_at IS NULL
          AND n.archived_at IS NULL
          AND n.created_at <= now()
        RETURNING n.id
        "#
    );
    let rows: Vec<(Uuid,)> = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(rows.len() as i64))
}

pub async fn get_prefs(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<NotificationPrefs, NotificationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_member(&mut tx, workspace_id, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let prefs = resolved_store_prefs(find_prefs_tx(&mut tx, workspace_id, user_id).await?);
    tx.commit().await?;
    Ok(Ok(NotificationPrefs {
        in_app: prefs.in_app,
        mail_immediate: prefs.mail_immediate,
        mail_digest: prefs.mail_digest,
    }))
}

pub async fn put_prefs(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    prefs: NotificationPrefs,
) -> Result<Result<NotificationPrefs, NotificationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_notification_member(
        &mut OperationTx::Postgres(&mut tx),
        workspace_id,
        user_id,
        session_id,
        true,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.notification_prefs (
            workspace_id, user_id, in_app, mail_immediate, mail_digest, updated_at
        ) VALUES ($1, $2, $3, $4, $5, now())
        ON CONFLICT (workspace_id, user_id) DO UPDATE
        SET in_app = EXCLUDED.in_app,
            mail_immediate = EXCLUDED.mail_immediate,
            mail_digest = EXCLUDED.mail_digest,
            updated_at = now()
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(prefs.in_app)
    .bind(prefs.mail_immediate)
    .bind(prefs.mail_digest)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(prefs))
}

pub async fn list_me_notifications(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    filter: NotificationFilter,
    cursor: Option<&str>,
    limit: i32,
) -> Result<Result<NotificationPage, NotificationDbError>, sqlx::Error> {
    let decoded = match cursor {
        None => None,
        Some(raw) => match decode_cursor(raw, filter) {
            Ok(cursor) => Some(cursor),
            Err(err) => return Ok(Err(err)),
        },
    };
    let memberships = crate::db::workspace::list_workspaces_for_user(pool, user_id).await?;
    let mut merged = Vec::new();
    let mut any_more = false;
    for workspace in memberships {
        let mut tx = pool.begin().await?;
        if let Err(err) = require_member(&mut tx, workspace.id, user_id, session_id).await? {
            tx.rollback().await?;
            if matches!(err, NotificationDbError::Forbidden) {
                return Ok(Err(err));
            }
            continue;
        }
        let prefs = resolved_store_prefs(find_prefs_tx(&mut tx, workspace.id, user_id).await?);
        if !prefs.in_app {
            tx.commit().await?;
            continue;
        }
        let page = list_page(
            &mut tx,
            workspace.id,
            user_id,
            filter,
            decoded.as_ref(),
            limit,
            None,
        )
        .await?;
        any_more = any_more || page.next_cursor.is_some();
        merged.extend(page.items);
        tx.commit().await?;
    }
    merged.sort_by(page_order);
    let has_more = any_more || merged.len() > limit as usize;
    merged.truncate(limit as usize);
    let next_cursor = if has_more {
        merged
            .last()
            .map(|item| encode_cursor(item.created_at, item.id, filter))
    } else {
        None
    };
    Ok(Ok(NotificationPage {
        items: merged,
        next_cursor,
    }))
}

// Fixed named statements shared by native SQLite and the owned remote stream.
// Scope flags bind unrestricted/document/task at ?3/?4/?5; no SQL rewriting.
const FAMILY_NOTIFICATION_PAGE: &str = r#"
SELECT n.id,n.workspace_id,n.event_id,n.verb,n.actor_user_id,n.target_type,n.target_id,
       n.payload,n.read_at,n.archived_at,n.created_at,n.user_id
FROM notifications n WHERE n.workspace_id=?1 AND n.user_id=?2
 AND (?3=1 OR (?4=1 AND (n.target_type='document' OR (n.target_type='comment' AND json_extract(n.payload,'$.documentId') IS NOT NULL)))
           OR (?5=1 AND (n.target_type='task' OR (n.target_type='comment' AND json_extract(n.payload,'$.taskId') IS NOT NULL))))
 AND ((?6='all' AND n.archived_at IS NULL) OR (?6='unread' AND n.read_at IS NULL AND n.archived_at IS NULL)
      OR (?6='archived' AND n.archived_at IS NOT NULL))
 AND (?7 IS NULL OR ((n.created_at/1000-CASE WHEN n.created_at%1000<0 THEN 1 ELSE 0 END),n.id)<(?7,?8))
ORDER BY (n.created_at/1000-CASE WHEN n.created_at%1000<0 THEN 1 ELSE 0 END) DESC,n.id DESC LIMIT ?9
"#;
const FAMILY_NOTIFICATION_COUNT: &str = r#"
SELECT count(*) FROM notifications n WHERE n.workspace_id=?1 AND n.user_id=?2
 AND (?3=1 OR (?4=1 AND (n.target_type='document' OR (n.target_type='comment' AND json_extract(n.payload,'$.documentId') IS NOT NULL)))
           OR (?5=1 AND (n.target_type='task' OR (n.target_type='comment' AND json_extract(n.payload,'$.taskId') IS NOT NULL))))
 AND n.read_at IS NULL AND n.archived_at IS NULL
"#;
const FAMILY_NOTIFICATION_FLAGS: &str = r#"
UPDATE notifications AS n
SET read_at=CASE WHEN ?7 IS NULL THEN read_at WHEN ?7=1 THEN ?9 ELSE NULL END,
    archived_at=CASE WHEN ?8 IS NULL THEN archived_at WHEN ?8=1 THEN ?9 ELSE NULL END
WHERE n.workspace_id=?1 AND n.user_id=?2 AND n.id=?6
 AND (?3=1 OR (?4=1 AND (n.target_type='document' OR (n.target_type='comment' AND json_extract(n.payload,'$.documentId') IS NOT NULL)))
           OR (?5=1 AND (n.target_type='task' OR (n.target_type='comment' AND json_extract(n.payload,'$.taskId') IS NOT NULL))))
RETURNING id
"#;
const FAMILY_NOTIFICATION_READ_ALL: &str = r#"
UPDATE notifications AS n SET read_at=?6
WHERE n.workspace_id=?1 AND n.user_id=?2
 AND (?3=1 OR (?4=1 AND (n.target_type='document' OR (n.target_type='comment' AND json_extract(n.payload,'$.documentId') IS NOT NULL)))
           OR (?5=1 AND (n.target_type='task' OR (n.target_type='comment' AND json_extract(n.payload,'$.taskId') IS NOT NULL))))
 AND n.read_at IS NULL AND n.archived_at IS NULL AND n.created_at<=?6 RETURNING id
"#;

fn notification_scope_cells(
    workspace: Uuid,
    user: Uuid,
    kinds: Option<&[ContentKind]>,
) -> Vec<Cell> {
    vec![
        Cell::uuid(workspace),
        Cell::uuid(user),
        Cell::Integer(i64::from(kinds.is_none())),
        Cell::Integer(i64::from(
            kinds.is_some_and(|k| k.contains(&ContentKind::Document)),
        )),
        Cell::Integer(i64::from(
            kinds.is_some_and(|k| k.contains(&ContentKind::Task)),
        )),
    ]
}
fn flag_cell(value: Option<bool>) -> Cell {
    value
        .map(|v| Cell::Integer(i64::from(v)))
        .unwrap_or(Cell::Null)
}
fn decode_notification_cells(
    cells: &[Cell],
    workspace: Uuid,
    user: Uuid,
) -> Result<NotificationRow, sqlx::Error> {
    if cells.len() != 12 || cells[1].id()? != workspace || cells[11].id()? != user {
        return Err(sqlx::Error::Protocol(
            "notification row tenant/recipient/width mismatch".into(),
        ));
    }
    Ok(NotificationRow {
        id: cells[0].id()?,
        workspace_id: workspace,
        event_id: cells[2].id()?,
        verb: cells[3].string()?,
        actor_user_id: cells[4].optional(Cell::id)?,
        actor_given_name: None,
        actor_family_name: None,
        target_type: cells[5].optional(Cell::string)?,
        target_id: cells[6].optional(Cell::id)?,
        display_id: None,
        payload: cells[7].value()?,
        read_at: cells[8].optional(Cell::datetime)?,
        archived_at: cells[9].optional(Cell::datetime)?,
        created_at: cells[10].datetime()?,
    })
}
fn decode_notification_row(
    row: FamilyRow,
    workspace: Uuid,
    user: Uuid,
) -> Result<NotificationRow, sqlx::Error> {
    let cells = (0..12)
        .map(|i| row.cell(i))
        .collect::<Result<Vec<_>, _>>()?;
    decode_notification_cells(&cells, workspace, user)
}

impl OperationTx<'_, '_> {
    pub(crate) async fn notification_prefs(
        &mut self,
        workspace: Uuid,
        user: Uuid,
    ) -> Result<Option<NotificationPrefs>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => find_prefs_tx(tx, workspace, user).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT in_app,mail_immediate,mail_digest FROM notification_prefs WHERE workspace_id=?1 AND user_id=?2",&[Cell::uuid(workspace),Cell::uuid(user)]).await?;
                rows.first()
                    .map(|r| {
                        Ok(NotificationPrefs {
                            in_app: r.cell(0)?.boolean()?,
                            mail_immediate: r.cell(1)?.boolean()?,
                            mail_digest: r.cell(2)?.boolean()?,
                        })
                    })
                    .transpose()
            }
        }
    }
    pub(crate) async fn insert_notifications(
        &mut self,
        rows: &[NotificationInsert],
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => insert_many(tx, rows).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                let now = Cell::Integer(Utc::now().timestamp_micros());
                for row in rows {
                    tx.require_tenant(row.workspace_id)?;
                    tx.execute("INSERT INTO notifications(id,workspace_id,user_id,event_id,verb,actor_user_id,target_type,target_id,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(workspace_id,user_id,event_id) DO NOTHING",
                        &[Cell::uuid(row.id),Cell::uuid(row.workspace_id),Cell::uuid(row.user_id),Cell::uuid(row.event_id),Cell::text(&row.verb),Cell::optional_uuid(row.actor_user_id),Cell::optional_text(row.target_type.as_deref()),Cell::optional_uuid(row.target_id),Cell::json(&row.payload)?,now.clone()]).await?;
                }
                Ok(())
            }
        }
    }
}

impl FamilyTx {
    async fn notification_page(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        filter: NotificationFilter,
        cursor: Option<&ListCursor>,
        limit: i32,
        kinds: Option<&[ContentKind]>,
    ) -> Result<NotificationPage, sqlx::Error> {
        self.require_tenant(workspace)?;
        let mut binds = notification_scope_cells(workspace, user, kinds);
        binds.extend([
            Cell::text(filter.as_str()),
            cursor
                .map(|c| Cell::Integer(c.ca.timestamp_millis()))
                .unwrap_or(Cell::Null),
            Cell::optional_uuid(cursor.map(|c| c.id)),
            Cell::Integer(i64::from(limit) + 1),
        ]);
        let rows = self.query(FAMILY_NOTIFICATION_PAGE, &binds).await?;
        let has_more = rows.len() > limit as usize;
        let mut items = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            let mut item = decode_notification_row(row, workspace, user)?;
            (item.actor_given_name, item.actor_family_name) =
                self.notification_actor_names(item.actor_user_id).await?;
            item.display_id = self
                .notification_display_id(
                    workspace,
                    item.target_type.as_deref(),
                    item.target_id,
                    &item.payload,
                )
                .await?;
            items.push(item);
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|i| encode_cursor(i.created_at, i.id, filter))
        } else {
            None
        };
        Ok(NotificationPage { items, next_cursor })
    }
    async fn notification_actor_names(
        &mut self,
        actor: Option<Uuid>,
    ) -> Result<(Option<String>, Option<String>), sqlx::Error> {
        let Some(actor) = actor else {
            return Ok((None, None));
        };
        let rows = self
            .query(
                "SELECT given_name,family_name FROM users WHERE id=?1 AND deleted_at IS NULL",
                &[Cell::uuid(actor)],
            )
            .await?;
        rows.first()
            .map(|r| {
                Ok((
                    Some(r.cell(0)?.string()?),
                    r.cell(1)?.optional(Cell::string)?,
                ))
            })
            .unwrap_or(Ok((None, None)))
    }
    async fn notification_display_id(
        &mut self,
        workspace: Uuid,
        target_type: Option<&str>,
        target_id: Option<Uuid>,
        payload: &Value,
    ) -> Result<Option<String>, sqlx::Error> {
        self.require_tenant(workspace)?;
        let Some((kind, id)) = display_target(target_type, target_id, payload) else {
            return Ok(None);
        };
        let binds = [Cell::uuid(workspace), Cell::uuid(id)];
        let rows=match kind {
            "task"=>self.query("SELECT p.key,t.number FROM tasks t JOIN projects p ON p.workspace_id=t.workspace_id AND p.id=t.project_id WHERE t.workspace_id=?1 AND t.id=?2",&binds).await?,
            "document"=>self.query("SELECT p.key,d.number,d.project_id FROM documents d LEFT JOIN projects p ON p.workspace_id=d.workspace_id AND p.id=d.project_id WHERE d.workspace_id=?1 AND d.id=?2",&binds).await?,
            "project"=>self.query("SELECT key FROM projects WHERE workspace_id=?1 AND id=?2",&binds).await?,
            _=>return Err(sqlx::Error::Protocol("invalid notification display kind".into()))
        };
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        if kind == "project" {
            return Ok(Some(row.cell(0)?.string()?));
        }
        let key = if kind == "document" && row.cell(2)?.optional(Cell::id)?.is_none() {
            Some("WIKI".into())
        } else {
            row.cell(0)?.optional(Cell::string)?
        };
        let number = i32::try_from(row.cell(1)?.integer()?).map_err(|_| {
            sqlx::Error::Protocol("notification display number out of range".into())
        })?;
        Ok(key.map(|k| format_display_id(&k, number)))
    }
    async fn notification_count(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        kinds: Option<&[ContentKind]>,
    ) -> Result<i64, sqlx::Error> {
        self.require_tenant(workspace)?;
        let rows = self
            .query(
                FAMILY_NOTIFICATION_COUNT,
                &notification_scope_cells(workspace, user, kinds),
            )
            .await?;
        let count = rows
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .integer()?;
        if count < 0 {
            return Err(sqlx::Error::Protocol("negative notification count".into()));
        }
        Ok(count)
    }
    async fn notification_set_flags(
        &mut self,
        input: &SetNotificationFlags<'_>,
    ) -> Result<bool, sqlx::Error> {
        self.require_writer()?;
        self.require_tenant(input.workspace_id)?;
        let mut binds =
            notification_scope_cells(input.workspace_id, input.user_id, input.allowed_kinds);
        binds.extend([
            Cell::uuid(input.notification_id),
            flag_cell(input.read),
            flag_cell(input.archived),
            Cell::Integer(Utc::now().timestamp_micros()),
        ]);
        let rows = self.query(FAMILY_NOTIFICATION_FLAGS, &binds).await?;
        for row in &rows {
            if row.cell(0)?.id()? != input.notification_id {
                return Err(sqlx::Error::Protocol(
                    "notification update target mismatch".into(),
                ));
            }
        }
        Ok(!rows.is_empty())
    }
    async fn notification_read_all(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        kinds: Option<&[ContentKind]>,
    ) -> Result<i64, sqlx::Error> {
        self.require_writer()?;
        self.require_tenant(workspace)?;
        let mut binds = notification_scope_cells(workspace, user, kinds);
        binds.push(Cell::Integer(Utc::now().timestamp_micros()));
        let rows = self.query(FAMILY_NOTIFICATION_READ_ALL, &binds).await?;
        for row in &rows {
            row.cell(0)?.id()?;
        }
        i64::try_from(rows.len())
            .map_err(|_| sqlx::Error::Protocol("notification update count overflow".into()))
    }
    async fn notification_put_prefs(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        prefs: NotificationPrefs,
    ) -> Result<(), sqlx::Error> {
        self.require_writer()?;
        self.require_tenant(workspace)?;
        self.execute("INSERT INTO notification_prefs(workspace_id,user_id,in_app,mail_immediate,mail_digest,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(workspace_id,user_id) DO UPDATE SET in_app=excluded.in_app,mail_immediate=excluded.mail_immediate,mail_digest=excluded.mail_digest,updated_at=excluded.updated_at",
            &[Cell::uuid(workspace),Cell::uuid(user),Cell::Integer(i64::from(prefs.in_app)),Cell::Integer(i64::from(prefs.mail_immediate)),Cell::Integer(i64::from(prefs.mail_digest)),Cell::Integer(Utc::now().timestamp_micros())]).await?;
        Ok(())
    }
}

#[cfg(test)]
mod backend_contract_tests {
    use super::*;
    fn cells(workspace: Uuid, user: Uuid, id: Uuid) -> Vec<Cell> {
        vec![
            Cell::uuid(id),
            Cell::uuid(workspace),
            Cell::uuid(Uuid::now_v7()),
            Cell::text("comment.created"),
            Cell::Null,
            Cell::text("comment"),
            Cell::Null,
            Cell::text("{\"parentId\":null,\"text\":\"한글🙂\"}"),
            Cell::Null,
            Cell::Null,
            Cell::Integer(-1),
            Cell::uuid(user),
        ]
    }
    #[test]
    fn notification_cells_keep_precision_nulls_and_reject_foreign_account() {
        let w = Uuid::now_v7();
        let u = Uuid::now_v7();
        let id = Uuid::now_v7();
        let good = cells(w, u, id);
        let row = decode_notification_cells(&good, w, u).unwrap();
        assert_eq!(row.id, id);
        assert_eq!(row.created_at.timestamp_micros(), -1);
        assert_eq!(row.actor_user_id, None);
        assert_eq!(row.read_at, None);
        assert_eq!(row.payload["parentId"], Value::Null);
        assert_eq!(row.payload["text"], "한글🙂");
        assert!(decode_notification_cells(&good, w, Uuid::now_v7()).is_err());
        assert!(decode_notification_cells(&good, Uuid::now_v7(), u).is_err());
        for (i, bad) in [
            (0, Cell::text(id.to_string())),
            (0, Cell::Blob(vec![0; 15])),
            (7, Cell::Null),
            (7, Cell::text("broken")),
            (8, Cell::text("2026-10-04")),
            (10, Cell::text("-1")),
        ] {
            let mut v = good.clone();
            v[i] = bad;
            assert!(decode_notification_cells(&v, w, u).is_err(), "column {i}");
        }
        assert!(decode_notification_cells(&good[..11], w, u).is_err());
    }
    #[test]
    fn global_merge_uses_cursor_millisecond_then_uuid_and_preserves_us() {
        let w = Uuid::now_v7();
        let u = Uuid::now_v7();
        let low = Uuid::from_u128(1);
        let high = Uuid::from_u128(2);
        let mut a = cells(w, u, low);
        a[10] = Cell::Integer(1999);
        let mut b = cells(w, u, high);
        b[10] = Cell::Integer(1001);
        let mut rows = vec![
            decode_notification_cells(&a, w, u).unwrap(),
            decode_notification_cells(&b, w, u).unwrap(),
        ];
        rows.sort_by(page_order);
        assert_eq!(rows[0].id, high);
        assert_eq!(rows[0].created_at.timestamp_micros(), 1001);
        let encoded = encode_cursor(rows[0].created_at, rows[0].id, NotificationFilter::All);
        let cursor = decode_cursor(&encoded, NotificationFilter::All).unwrap();
        assert_eq!(cursor.ca.timestamp_millis(), 1);
        assert_eq!(cursor.id, high);
        assert!(decode_cursor(&encoded, NotificationFilter::Unread).is_err());
        let pre = decode_cursor(
            &encode_cursor(
                DateTime::from_timestamp_micros(-1).unwrap(),
                low,
                NotificationFilter::All,
            ),
            NotificationFilter::All,
        )
        .unwrap();
        assert_eq!(pre.ca.timestamp_millis(), -1);
    }
    #[test]
    fn empty_pat_content_scope_is_distinct_from_unrestricted_and_comment_target_is_shared() {
        let w = Uuid::now_v7();
        let u = Uuid::now_v7();
        assert_eq!(
            &notification_scope_cells(w, u, None)[2..],
            &[Cell::Integer(1), Cell::Integer(0), Cell::Integer(0)]
        );
        assert_eq!(
            &notification_scope_cells(w, u, Some(&[]))[2..],
            &[Cell::Integer(0), Cell::Integer(0), Cell::Integer(0)]
        );
        assert_eq!(
            &notification_scope_cells(w, u, Some(&[ContentKind::Document]))[2..],
            &[Cell::Integer(0), Cell::Integer(1), Cell::Integer(0)]
        );
        let task = Uuid::now_v7();
        let doc = Uuid::now_v7();
        let payload = serde_json::json!({"taskId":task,"documentId":doc,"parentId":null});
        assert_eq!(
            display_target(Some("comment"), Some(Uuid::now_v7()), &payload),
            Some(("task", task))
        );
        assert_eq!(
            display_target(Some("document"), Some(doc), &payload),
            Some(("document", doc))
        );
    }
}

pub async fn list_notifications_backend(
    backend: &Backend,
    query: ListNotificationsQuery<'_>,
) -> Result<Result<NotificationPage, NotificationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_notifications(pool, query).await;
    }
    if !(1..=100).contains(&query.limit) {
        return Ok(Err(NotificationDbError::InvalidInput));
    }
    let cursor = match query.cursor {
        Some(raw) => match decode_cursor(raw, query.filter) {
            Ok(c) => Some(c),
            Err(e) => return Ok(Err(e)),
        },
        None => None,
    };
    let mut tx = backend.begin_read().await?;
    if let Err(e) = require_notification_member(
        &mut tx.operation(),
        query.workspace_id,
        query.user_id,
        query.session_id,
        false,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let prefs = resolved_store_prefs(
        tx.operation()
            .notification_prefs(query.workspace_id, query.user_id)
            .await?,
    );
    let result = if prefs.in_app {
        let OperationTx::SqliteFamily(family) = tx.operation() else {
            unreachable!("PG handled above")
        };
        family
            .notification_page(
                query.workspace_id,
                query.user_id,
                query.filter,
                cursor.as_ref(),
                query.limit,
                query.allowed_kinds,
            )
            .await?
    } else {
        NotificationPage {
            items: Vec::new(),
            next_cursor: None,
        }
    };
    tx.rollback().await?;
    Ok(Ok(result))
}
pub async fn unread_count_backend(
    backend: &Backend,
    workspace: Uuid,
    user: Uuid,
    credential: Uuid,
    kinds: Option<&[ContentKind]>,
) -> Result<Result<i64, NotificationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return unread_count(pool, workspace, user, credential, kinds).await;
    }
    let mut tx = backend.begin_read().await?;
    if let Err(e) =
        require_notification_member(&mut tx.operation(), workspace, user, credential, false).await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let prefs = resolved_store_prefs(tx.operation().notification_prefs(workspace, user).await?);
    let result = if prefs.in_app {
        let OperationTx::SqliteFamily(family) = tx.operation() else {
            unreachable!("PG handled above")
        };
        family.notification_count(workspace, user, kinds).await?
    } else {
        0
    };
    tx.rollback().await?;
    Ok(Ok(result))
}
pub async fn set_flags_backend(
    backend: &Backend,
    input: SetNotificationFlags<'_>,
) -> Result<Result<bool, NotificationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return set_flags(pool, input).await;
    }
    if input.read.is_none() && input.archived.is_none() {
        return Ok(Err(NotificationDbError::InvalidInput));
    }
    let mut tx = backend.begin_write().await?;
    if let Err(e) = require_notification_member(
        &mut tx.operation(),
        input.workspace_id,
        input.user_id,
        input.session_id,
        true,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("PG handled above")
    };
    let updated = family.notification_set_flags(&input).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(Ok(updated))
}
pub async fn read_all_backend(
    backend: &Backend,
    workspace: Uuid,
    user: Uuid,
    credential: Uuid,
    kinds: Option<&[ContentKind]>,
) -> Result<Result<i64, NotificationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return read_all(pool, workspace, user, credential, kinds).await;
    }
    let mut tx = backend.begin_write().await?;
    if let Err(e) =
        require_notification_member(&mut tx.operation(), workspace, user, credential, true).await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("PG handled above")
    };
    let updated = family.notification_read_all(workspace, user, kinds).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(Ok(updated))
}
pub async fn get_prefs_backend(
    backend: &Backend,
    workspace: Uuid,
    user: Uuid,
    credential: Uuid,
) -> Result<Result<NotificationPrefs, NotificationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return get_prefs(pool, workspace, user, credential).await;
    }
    let mut tx = backend.begin_read().await?;
    if let Err(e) =
        require_notification_member(&mut tx.operation(), workspace, user, credential, false).await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let prefs = resolved_store_prefs(tx.operation().notification_prefs(workspace, user).await?);
    tx.rollback().await?;
    Ok(Ok(prefs))
}
pub async fn put_prefs_backend(
    backend: &Backend,
    workspace: Uuid,
    user: Uuid,
    credential: Uuid,
    prefs: NotificationPrefs,
) -> Result<Result<NotificationPrefs, NotificationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return put_prefs(pool, workspace, user, credential, prefs).await;
    }
    let mut tx = backend.begin_write().await?;
    if let Err(e) =
        require_notification_member(&mut tx.operation(), workspace, user, credential, true).await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("PG handled above")
    };
    family
        .notification_put_prefs(workspace, user, prefs)
        .await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(Ok(prefs))
}
pub async fn list_me_notifications_backend(
    backend: &Backend,
    user: Uuid,
    credential: Uuid,
    filter: NotificationFilter,
    cursor: Option<&str>,
    limit: i32,
) -> Result<Result<NotificationPage, NotificationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_me_notifications(pool, user, credential, filter, cursor, limit).await;
    }
    if !(1..=100).contains(&limit) {
        return Ok(Err(NotificationDbError::InvalidInput));
    }
    // Validate before membership discovery, including an empty workspace list.
    if let Some(raw) = cursor {
        if let Err(e) = decode_cursor(raw, filter) {
            return Ok(Err(e));
        }
    }
    let mut tx = backend.begin_read().await?;
    let previous = tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("PG handled above")
    };
    family.require_system_context()?;
    let rows=family.query("SELECT w.id FROM memberships m JOIN workspaces w ON w.id=m.workspace_id JOIN users u ON u.id=m.user_id WHERE m.user_id=?1 AND w.deleted_at IS NULL AND u.deleted_at IS NULL AND u.suspended_at IS NULL ORDER BY w.id",&[Cell::uuid(user)]).await?;
    let workspaces = rows
        .iter()
        .map(|r| r.cell(0)?.id())
        .collect::<Result<Vec<_>, _>>()?;
    tx.operation().restore_system(previous).await?;
    tx.rollback().await?;
    let mut merged = Vec::new();
    let mut any_more = false;
    for workspace in workspaces {
        match list_notifications_backend(
            backend,
            ListNotificationsQuery {
                workspace_id: workspace,
                user_id: user,
                session_id: credential,
                filter,
                cursor,
                limit,
                allowed_kinds: None,
            },
        )
        .await?
        {
            Ok(page) => {
                any_more |= page.next_cursor.is_some();
                merged.extend(page.items);
            }
            Err(NotificationDbError::NotFound) => continue,
            Err(e) => return Ok(Err(e)),
        }
    }
    merged.sort_by(page_order);
    let more = any_more || merged.len() > limit as usize;
    merged.truncate(limit as usize);
    let next_cursor = if more {
        merged
            .last()
            .map(|i| encode_cursor(i.created_at, i.id, filter))
    } else {
        None
    };
    Ok(Ok(NotificationPage {
        items: merged,
        next_cursor,
    }))
}

#[cfg(test)]
pub(crate) mod family_runtime_fixture {
    use super::*;
    pub(crate) struct Fixture {
        pub backend: Backend,
        pub pool: sqlx::SqlitePool,
        pub workspace: Uuid,
        pub other_workspace: Uuid,
        pub actor: Uuid,
        pub user: Uuid,
        pub credential: Uuid,
        pub other_user: Uuid,
        pub other_credential: Uuid,
        pub document: Uuid,
        pub comment: Uuid,
        pub dir: std::path::PathBuf,
    }
    impl Fixture {
        pub(crate) async fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("fvoci-notification-{}", Uuid::now_v7()));
            std::fs::create_dir(&dir).unwrap();
            let file = dir.join("test.sqlite");
            let prepare = crate::db::pool::connect_sqlite_prepare(&file)
                .await
                .unwrap();
            let mut tx = prepare.begin_with("BEGIN IMMEDIATE").await.unwrap();
            for ddl in [
                include_str!("../../migrations/sqlite/001_current_schema.sql"),
                include_str!("../../migrations/sqlite/002_wiki_create_commands.sql"),
                include_str!("../../migrations/sqlite/003_collab_room_fences.sql"),
            ] {
                sqlx::raw_sql(ddl).execute(&mut *tx).await.unwrap();
            }
            tx.commit().await.unwrap();
            prepare.close().await;
            let pool = crate::db::pool::connect_sqlite_app(&file, 1).await.unwrap();
            let workspace = Uuid::now_v7();
            let other_workspace = Uuid::now_v7();
            let actor = Uuid::now_v7();
            let user = Uuid::now_v7();
            let other_user = Uuid::now_v7();
            let credential = Uuid::now_v7();
            let other_credential = Uuid::now_v7();
            let document = Uuid::now_v7();
            let comment = Uuid::now_v7();
            for (id, email) in [
                (actor, "actor@notification.invalid"),
                (user, "reader@notification.invalid"),
                (other_user, "other@notification.invalid"),
            ] {
                sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'한글🙂')")
                    .bind(id.as_bytes().as_slice())
                    .bind(email)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            for (id, slug) in [
                (workspace, "notify-main"),
                (other_workspace, "notify-other"),
            ] {
                sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,?2,'team')")
                    .bind(id.as_bytes().as_slice())
                    .bind(slug)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            for (w, u, role) in [
                (workspace, actor, "owner"),
                (workspace, user, "guest"),
                (other_workspace, other_user, "owner"),
            ] {
                sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)")
                    .bind(w.as_bytes().as_slice())
                    .bind(u.as_bytes().as_slice())
                    .bind(role)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            for (id, u, hash) in [
                (credential, user, "notification-reader"),
                (other_credential, other_user, "notification-other"),
            ] {
                sqlx::query(
                    "INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,?4)",
                )
                .bind(id.as_bytes().as_slice())
                .bind(u.as_bytes().as_slice())
                .bind(hash)
                .bind(Utc::now().timestamp_micros() + 3_600_000_000i64)
                .execute(&pool)
                .await
                .unwrap();
            }
            sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'private wiki',?3,'a',1,'published',2,?4,'{\"type\":\"doc\"}')")
                .bind(document.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(document.simple().to_string()).bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO comments(id,workspace_id,document_id,created_by,body) VALUES(?1,?2,?3,?4,'comment 한글🙂')")
                .bind(comment.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).execute(&pool).await.unwrap();
            Self {
                backend: Backend::Sqlite(pool.clone()),
                pool,
                workspace,
                other_workspace,
                actor,
                user,
                credential,
                other_user,
                other_credential,
                document,
                comment,
                dir,
            }
        }
        pub(crate) async fn grant_wiki(&self) {
            let group = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'notification group')",
            )
            .bind(group.as_bytes().as_slice())
            .bind(self.workspace.as_bytes().as_slice())
            .execute(&self.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)",
            )
            .bind(self.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(self.user.as_bytes().as_slice())
            .execute(&self.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(self.workspace.as_bytes().as_slice()).bind(self.document.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&self.pool).await.unwrap();
        }
        pub(crate) async fn append_comment_event(
            &self,
            verb: &str,
        ) -> crate::db::outbox::BackendOutboxEvent {
            let id = Uuid::now_v7();
            let mut tx = self.backend.begin_write().await.unwrap();
            tx.operation().set_tenant(self.workspace).await.unwrap();
            let previous = tx.operation().set_system().await.unwrap();
            tx.operation().append_event(crate::db::identity::EventAppend{id,workspace_id:Some(self.workspace),actor_user_id:Some(self.actor),verb:verb.into(),target_type:Some("comment".into()),target_id:Some(self.comment),payload:serde_json::json!({"commentId":self.comment,"mentionedUserIds":[self.user]})}).await.unwrap();
            tx.operation().restore_system(previous).await.unwrap();
            tx.commit().await.unwrap();
            crate::db::outbox::fetch_event_by_id_backend(&self.backend, id)
                .await
                .unwrap()
                .unwrap()
        }
        pub(crate) async fn insert_inbox(&self, id: Uuid, at: i64) {
            let mut tx = self.backend.begin_write().await.unwrap();
            tx.operation().set_tenant(self.workspace).await.unwrap();
            let previous = tx.operation().set_system().await.unwrap();
            tx.operation()
                .insert_notifications(&[NotificationInsert {
                    id,
                    workspace_id: self.workspace,
                    user_id: self.user,
                    event_id: Uuid::now_v7(),
                    verb: "document.updated".into(),
                    actor_user_id: Some(self.actor),
                    target_type: Some("document".into()),
                    target_id: Some(self.document),
                    payload: serde_json::json!({"text":"한글🙂","parentId":null}),
                }])
                .await
                .unwrap();
            tx.operation().restore_system(previous).await.unwrap();
            tx.commit().await.unwrap();
            sqlx::query("UPDATE notifications SET created_at=?2 WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .bind(at)
                .execute(&self.pool)
                .await
                .unwrap();
        }
        pub(crate) async fn finish(self) {
            self.backend.close().await.unwrap();
            std::fs::remove_dir_all(&self.dir).unwrap();
        }
    }
    #[tokio::test]
    async fn actual_backend_inbox_cursor_flags_scope_and_current_authority() {
        let f = Fixture::new().await;
        let low = Uuid::from_u128(1);
        let high = Uuid::from_u128(2);
        f.insert_inbox(low, 1999).await;
        f.insert_inbox(high, 1001).await;
        let query = ListNotificationsQuery {
            workspace_id: f.workspace,
            user_id: f.user,
            session_id: f.credential,
            filter: NotificationFilter::All,
            cursor: None,
            limit: 1,
            allowed_kinds: None,
        };
        let a = list_notifications_backend(&f.backend, query)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(a.items[0].id, high);
        assert_eq!(a.items[0].created_at.timestamp_micros(), 1001);
        assert_eq!(a.items[0].display_id.as_deref(), Some("WIKI-1"));
        let b = list_notifications_backend(
            &f.backend,
            ListNotificationsQuery {
                workspace_id: f.workspace,
                user_id: f.user,
                session_id: f.credential,
                filter: NotificationFilter::All,
                cursor: a.next_cursor.as_deref(),
                limit: 1,
                allowed_kinds: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(b.items[0].id, low);
        assert_eq!(b.items[0].created_at.timestamp_micros(), 1999);
        assert!(b.next_cursor.is_none());
        assert_eq!(
            unread_count_backend(&f.backend, f.workspace, f.user, f.credential, None)
                .await
                .unwrap()
                .unwrap(),
            2
        );
        assert_eq!(
            unread_count_backend(&f.backend, f.workspace, f.user, f.credential, Some(&[]))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        let flags = |user, credential| SetNotificationFlags {
            workspace_id: f.workspace,
            user_id: user,
            session_id: credential,
            notification_id: high,
            read: Some(true),
            archived: None,
            allowed_kinds: None,
        };
        assert!(matches!(
            set_flags_backend(&f.backend, flags(f.other_user, f.other_credential))
                .await
                .unwrap(),
            Err(NotificationDbError::NotFound)
        ));
        assert!(matches!(
            set_flags_backend(&f.backend, flags(f.user, f.other_credential))
                .await
                .unwrap(),
            Err(NotificationDbError::Forbidden)
        ));
        assert!(set_flags_backend(&f.backend, flags(f.user, f.credential))
            .await
            .unwrap()
            .unwrap());
        assert_eq!(
            read_all_backend(&f.backend, f.workspace, f.user, f.credential, None)
                .await
                .unwrap()
                .unwrap(),
            1
        );
        assert_eq!(
            unread_count_backend(&f.backend, f.workspace, f.user, f.credential, None)
                .await
                .unwrap()
                .unwrap(),
            0
        );
        sqlx::query("UPDATE sessions SET revoked_at=?2 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .bind(Utc::now().timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            read_all_backend(&f.backend, f.workspace, f.user, f.credential, None)
                .await
                .unwrap(),
            Err(NotificationDbError::Forbidden)
        ));
        f.finish().await;
    }
    #[tokio::test]
    async fn actual_backend_prefs_membership_and_tenant_denials() {
        let f = Fixture::new().await;
        let default = get_prefs_backend(&f.backend, f.workspace, f.user, f.credential)
            .await
            .unwrap()
            .unwrap();
        assert!(default.in_app && default.mail_immediate && !default.mail_digest);
        let disabled = NotificationPrefs {
            in_app: false,
            mail_immediate: false,
            mail_digest: true,
        };
        let stored = put_prefs_backend(&f.backend, f.workspace, f.user, f.credential, disabled)
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.in_app && !stored.mail_immediate && stored.mail_digest);
        f.insert_inbox(Uuid::now_v7(), Utc::now().timestamp_micros())
            .await;
        let page = list_me_notifications_backend(
            &f.backend,
            f.user,
            f.credential,
            NotificationFilter::All,
            None,
            10,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(page.items.is_empty());
        assert!(matches!(
            get_prefs_backend(&f.backend, f.other_workspace, f.user, f.credential)
                .await
                .unwrap(),
            Err(NotificationDbError::NotFound)
        ));
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            put_prefs_backend(&f.backend, f.workspace, f.user, f.credential, DEFAULT_PREFS)
                .await
                .unwrap(),
            Err(NotificationDbError::NotFound)
        ));
        let prefs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_prefs")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            prefs, 0,
            "membership loss must not recreate an inbox preference"
        );
        f.finish().await;
    }
}

// Explicit notification source reads on the caller-owned transaction.
pub(crate) struct CommentSnap {
    pub(crate) id: Uuid,
    pub(crate) document_id: Option<Uuid>,
    pub(crate) task_id: Option<Uuid>,
    pub(crate) parent_id: Option<Uuid>,
    pub(crate) created_by: Uuid,
    pub(crate) body: String,
}

pub(crate) struct TaskSnap {
    pub(crate) number: i32,
    pub(crate) title: String,
    pub(crate) project_id: Uuid,
}

async fn user_is_present_pg(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let present: Option<(bool,)> =
        sqlx::query_as("SELECT true FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(present.is_some())
}

async fn list_task_assignees_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT user_id
        FROM fvoci.task_assignees
        WHERE workspace_id = $1 AND task_id = $2
        ORDER BY user_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn task_snapshot_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<TaskSnap>, sqlx::Error> {
    let row: Option<(i32, String, Uuid)> = sqlx::query_as(
        r#"
        SELECT number, title, project_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(number, title, project_id)| TaskSnap {
        number,
        title,
        project_id,
    }))
}

async fn status_name_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    status_id: &str,
) -> Result<String, sqlx::Error> {
    let Ok(id) = Uuid::parse_str(status_id) else {
        return Ok(status_id.to_string());
    };
    let row: Option<(String,)> =
        sqlx::query_as("SELECT name FROM fvoci.statuses WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row
        .map(|(name,)| name)
        .unwrap_or_else(|| status_id.to_string()))
}

pub(crate) async fn load_comment_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    comment_id: Uuid,
) -> Result<Option<CommentSnap>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, document_id, task_id, parent_id, created_by, body
        FROM fvoci.comments
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(comment_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| CommentSnap {
        id: row.get("id"),
        document_id: row.get("document_id"),
        task_id: row.get("task_id"),
        parent_id: row.get("parent_id"),
        created_by: row.get("created_by"),
        body: row.get("body"),
    }))
}

pub(crate) async fn user_is_present(
    tx: &mut OperationTx<'_, '_>,
    user: Uuid,
) -> Result<bool, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => user_is_present_pg(tx, user).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_system_context()?;
            let rows = tx
                .query(
                    "SELECT 1 FROM users WHERE id=?1 AND deleted_at IS NULL",
                    &[Cell::uuid(user)],
                )
                .await?;
            Ok(!rows.is_empty())
        }
    }
}

pub(crate) async fn list_task_assignees(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    task: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => list_task_assignees_pg(tx, workspace, task).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            tx.query("SELECT user_id FROM task_assignees WHERE workspace_id=?1 AND task_id=?2 ORDER BY user_id",&[Cell::uuid(workspace),Cell::uuid(task)]).await?.iter().map(|r|r.cell(0)?.id()).collect()
        }
    }
}

pub(crate) async fn task_snapshot(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    task: Uuid,
) -> Result<Option<TaskSnap>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => task_snapshot_pg(tx, workspace, task).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let rows=tx.query("SELECT number,title,project_id FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(task)]).await?;
            rows.first()
                .map(|r| {
                    Ok(TaskSnap {
                        number: i32::try_from(r.cell(0)?.integer()?).map_err(|_| {
                            sqlx::Error::Protocol("notification task number out of range".into())
                        })?,
                        title: r.cell(1)?.string()?,
                        project_id: r.cell(2)?.id()?,
                    })
                })
                .transpose()
        }
    }
}

pub(crate) async fn status_name(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    status: &str,
) -> Result<String, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => status_name_pg(tx, workspace, status).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let Ok(id) = Uuid::parse_str(status) else {
                return Ok(status.to_owned());
            };
            let rows = tx
                .query(
                    "SELECT name FROM statuses WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(id)],
                )
                .await?;
            rows.first()
                .map(|r| r.cell(0)?.string())
                .unwrap_or_else(|| Ok(status.to_owned()))
        }
    }
}

pub(crate) async fn load_comment(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    comment: Uuid,
) -> Result<Option<CommentSnap>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => load_comment_pg(tx, workspace, comment).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let rows=tx.query("SELECT id,document_id,task_id,parent_id,created_by,body FROM comments WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(workspace),Cell::uuid(comment)]).await?;
            rows.first()
                .map(|r| {
                    Ok(CommentSnap {
                        id: r.cell(0)?.id()?,
                        document_id: r.cell(1)?.optional(Cell::id)?,
                        task_id: r.cell(2)?.optional(Cell::id)?,
                        parent_id: r.cell(3)?.optional(Cell::id)?,
                        created_by: r.cell(4)?.id()?,
                        body: r.cell(5)?.string()?,
                    })
                })
                .transpose()
        }
    }
}

pub(crate) async fn notification_project_name(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    project: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx)=>sqlx::query_scalar("SELECT name FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL").bind(workspace).bind(project).fetch_optional(&mut ***tx).await,
        OperationTx::SqliteFamily(tx)=>{
            tx.require_tenant(workspace)?;tx.require_system_context()?;
            let rows=tx.query("SELECT name FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(project)]).await?;
            rows.first().map(|r|r.cell(0)?.string()).transpose()
        }
    }
}

pub(crate) async fn notification_group_members(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    group: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => {
            sqlx::query_scalar(
                "SELECT user_id FROM fvoci.group_members WHERE workspace_id=$1 AND group_id=$2",
            )
            .bind(workspace)
            .bind(group)
            .fetch_all(&mut ***tx)
            .await
        }
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            tx.query(
                "SELECT user_id FROM group_members WHERE workspace_id=?1 AND group_id=?2",
                &[Cell::uuid(workspace), Cell::uuid(group)],
            )
            .await?
            .iter()
            .map(|r| r.cell(0)?.id())
            .collect()
        }
    }
}

pub(crate) async fn notification_document_creator(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    document: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => {
            sqlx::query_scalar(
                "SELECT created_by FROM fvoci.documents WHERE workspace_id=$1 AND id=$2",
            )
            .bind(workspace)
            .bind(document)
            .fetch_optional(&mut ***tx)
            .await
        }
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let rows = tx
                .query(
                    "SELECT created_by FROM documents WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(document)],
                )
                .await?;
            rows.first().map(|r| r.cell(0)?.id()).transpose()
        }
    }
}

pub(crate) async fn notification_inviter(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    invitation: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => {
            sqlx::query_scalar(
                "SELECT invited_by FROM fvoci.invitations WHERE workspace_id=$1 AND id=$2",
            )
            .bind(workspace)
            .bind(invitation)
            .fetch_optional(&mut ***tx)
            .await
        }
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let rows = tx
                .query(
                    "SELECT invited_by FROM invitations WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(invitation)],
                )
                .await?;
            rows.first().map(|r| r.cell(0)?.id()).transpose()
        }
    }
}

pub(crate) async fn notification_document_project(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    document: Uuid,
) -> Result<Option<Option<Uuid>>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => sqlx::query_scalar(
            "SELECT project_id FROM fvoci.documents WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL"
        ).bind(workspace).bind(document).fetch_optional(&mut ***tx).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            tx.query("SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
                &[Cell::uuid(workspace), Cell::uuid(document)]).await?
                .first().map(|row| row.cell(0)?.optional(Cell::id)).transpose()
        }
    }
}
