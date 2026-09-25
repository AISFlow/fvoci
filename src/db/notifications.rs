use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::context::{session_is_live, set_self_user, set_tenant};
use crate::db::workspace::membership_role;
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
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), NotificationDbError>, sqlx::Error> {
    set_tenant(tx, workspace_id).await?;
    set_self_user(tx, user_id).await?;
    if !session_is_live(tx, user_id, session_id).await? {
        return Ok(Err(NotificationDbError::Forbidden));
    }
    if membership_role(tx, workspace_id, user_id).await?.is_none() {
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

async fn display_id_for(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target_type: Option<&str>,
    target_id: Option<Uuid>,
    payload: &Value,
) -> Result<Option<String>, sqlx::Error> {
    if let Some(target_id) = target_id {
        match target_type {
            Some("task") => return task_display_id(tx, workspace_id, target_id).await,
            Some("document") => return document_display_id(tx, workspace_id, target_id).await,
            Some("project") => return project_display_id(tx, workspace_id, target_id).await,
            _ => {}
        }
    }
    if target_type == Some("comment") {
        if let Some(task_id) = payload_uuid(payload, "taskId") {
            return task_display_id(tx, workspace_id, task_id).await;
        }
        if let Some(document_id) = payload_uuid(payload, "documentId") {
            return document_display_id(tx, workspace_id, document_id).await;
        }
    }
    Ok(None)
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
    if let Err(err) = require_member(&mut tx, workspace_id, user_id, session_id).await? {
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
    if let Err(err) = require_member(&mut tx, workspace_id, user_id, session_id).await? {
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
    if let Err(err) = require_member(&mut tx, workspace_id, user_id, session_id).await? {
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
    merged.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
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
