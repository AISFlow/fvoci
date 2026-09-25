//! Stars (per-user favourites) and the recently updated list.
//!
//! Source `packages/db/src/star-access.ts` + `core/star.ts` + `repos/search.ts
//! listRecent`: every read joins the target and re-applies the actor's current
//! read access, so a star on a document or task the actor can no longer read
//! stays in the table but is never returned.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::{membership_role, workspace_is_live};
use crate::db::notifications::ContentKind;
use crate::search::query::{load_search_acl, SearchAcl};

pub const RECENT_DEFAULT_LIMIT: i64 = 20;
pub const RECENT_MAX_LIMIT: i64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarTarget {
    Document(Uuid),
    Task(Uuid),
}

impl StarTarget {
    fn kind(self) -> ContentKind {
        match self {
            Self::Document(_) => ContentKind::Document,
            Self::Task(_) => ContentKind::Task,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarItem {
    pub id: Uuid,
    pub kind: ContentKind,
    pub target_id: Uuid,
    pub title: String,
    pub project_id: Option<Uuid>,
    pub number: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentItem {
    pub kind: ContentKind,
    pub id: Uuid,
    pub title: String,
    pub project_id: Option<Uuid>,
    pub number: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarDbError {
    NotFound,
    Forbidden,
}

pub fn kind_str(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Document => "document",
        ContentKind::Task => "task",
    }
}

fn parse_kind(value: &str) -> ContentKind {
    if value == "task" {
        ContentKind::Task
    } else {
        ContentKind::Document
    }
}

/// `None` = unrestricted (session). A token with neither read scope gets `[]`.
fn kind_allowed(kinds: Option<&[ContentKind]>, kind: ContentKind) -> bool {
    kinds.is_none_or(|kinds| kinds.contains(&kind))
}

/// Session/token live, workspace live, actor a member. Returns the read scope.
async fn require_member_acl(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    lock_for_write: bool,
) -> Result<Result<SearchAcl, StarDbError>, sqlx::Error> {
    set_tenant(tx, workspace_id).await?;
    let live = if lock_for_write {
        lock_membership_users(tx, &[actor_user_id]).await?;
        recheck_session(tx, actor_user_id, credential_id).await?
    } else {
        session_is_live(tx, actor_user_id, credential_id).await?
    };
    if !live {
        return Ok(Err(StarDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(StarDbError::NotFound));
    }
    let Some(role) = membership_role(tx, workspace_id, actor_user_id).await? else {
        return Ok(Err(StarDbError::Forbidden));
    };
    Ok(Ok(load_search_acl(
        tx,
        workspace_id,
        actor_user_id,
        role,
        None,
    )
    .await?))
}

/// Read predicate on a document row aliased `d`: `$2` project ids, `$3` include
/// wiki, `$4` guest wiki ids. Mirrors search `visible_after_hydrate`.
const DOCUMENT_READ_SQL: &str = "(
    (d.project_id IS NOT NULL AND d.project_id = ANY($2))
    OR (d.project_id IS NULL AND ($3 OR d.id = ANY($4)))
)";

type RecentRow = (String, Uuid, String, Option<Uuid>, i32, DateTime<Utc>);

type StarRow = (Uuid, String, Uuid, String, Option<Uuid>, i32, DateTime<Utc>);

async fn select_stars(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    acl: &SearchAcl,
    kinds: Option<&[ContentKind]>,
    only_id: Option<Uuid>,
) -> Result<Vec<StarItem>, sqlx::Error> {
    let sql = format!(
        r#"
        SELECT id, type, target_id, title, project_id, number, created_at FROM (
            SELECT s.id, 'document'::text AS type, d.id AS target_id, d.title,
                   d.project_id, d.number, s.created_at
            FROM fvoci.stars s
            INNER JOIN fvoci.documents d
                ON d.workspace_id = s.workspace_id AND d.id = s.document_id
            WHERE s.workspace_id = $1 AND s.user_id = $5
              AND $6 AND d.deleted_at IS NULL
              AND ($7::uuid IS NULL OR s.id = $7)
              AND {DOCUMENT_READ_SQL}
            UNION ALL
            SELECT s.id, 'task'::text AS type, t.id AS target_id, t.title,
                   t.project_id, t.number, s.created_at
            FROM fvoci.stars s
            INNER JOIN fvoci.tasks t
                ON t.workspace_id = s.workspace_id AND t.id = s.task_id
            WHERE s.workspace_id = $1 AND s.user_id = $5
              AND $8 AND t.deleted_at IS NULL
              AND ($7::uuid IS NOT NULL OR t.archived_at IS NULL)
              AND ($7::uuid IS NULL OR s.id = $7)
              AND t.project_id = ANY($2)
        ) u
        ORDER BY created_at DESC, id DESC
        "#
    );
    let rows: Vec<StarRow> = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(&acl.project_ids)
        .bind(acl.include_wiki)
        .bind(&acl.wiki_document_ids)
        .bind(actor_user_id)
        .bind(kind_allowed(kinds, ContentKind::Document))
        .bind(only_id)
        .bind(kind_allowed(kinds, ContentKind::Task))
        .fetch_all(&mut **tx)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, kind, target_id, title, project_id, number, created_at)| StarItem {
                id,
                kind: parse_kind(&kind),
                target_id,
                title,
                project_id,
                number,
                created_at,
            },
        )
        .collect())
}

pub async fn list_stars(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    kinds: Option<&[ContentKind]>,
) -> Result<Result<Vec<StarItem>, StarDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let acl = match require_member_acl(&mut tx, workspace_id, actor_user_id, credential_id, false)
        .await?
    {
        Ok(acl) => acl,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let items = select_stars(&mut tx, workspace_id, actor_user_id, &acl, kinds, None).await?;
    tx.commit().await?;
    Ok(Ok(items))
}

/// Idempotent: starring the same target again returns the existing star.
pub async fn add_star(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    target: StarTarget,
    kinds: Option<&[ContentKind]>,
) -> Result<Result<StarItem, StarDbError>, sqlx::Error> {
    if !kind_allowed(kinds, target.kind()) {
        return Ok(Err(StarDbError::NotFound));
    }
    let mut tx = pool.begin().await?;
    let acl = match require_member_acl(&mut tx, workspace_id, actor_user_id, credential_id, true)
        .await?
    {
        Ok(acl) => acl,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let inserted: Option<(Uuid,)> = match target {
        StarTarget::Document(document_id) => {
            let sql = format!(
                r#"
                INSERT INTO fvoci.stars (id, workspace_id, user_id, document_id)
                SELECT $6, $1, $5, d.id
                FROM fvoci.documents d
                WHERE d.workspace_id = $1 AND d.id = $7 AND d.deleted_at IS NULL
                  AND {DOCUMENT_READ_SQL}
                ON CONFLICT (workspace_id, user_id, document_id)
                    DO UPDATE SET document_id = EXCLUDED.document_id
                RETURNING id
                "#
            );
            sqlx::query_as(&sql)
                .bind(workspace_id)
                .bind(&acl.project_ids)
                .bind(acl.include_wiki)
                .bind(&acl.wiki_document_ids)
                .bind(actor_user_id)
                .bind(Uuid::now_v7())
                .bind(document_id)
                .fetch_optional(&mut *tx)
                .await?
        }
        StarTarget::Task(task_id) => {
            sqlx::query_as(
                r#"
                INSERT INTO fvoci.stars (id, workspace_id, user_id, task_id)
                SELECT $4, $1, $3, t.id
                FROM fvoci.tasks t
                WHERE t.workspace_id = $1 AND t.id = $5 AND t.deleted_at IS NULL
                  AND t.project_id = ANY($2)
                ON CONFLICT (workspace_id, user_id, task_id)
                    DO UPDATE SET task_id = EXCLUDED.task_id
                RETURNING id
                "#,
            )
            .bind(workspace_id)
            .bind(&acl.project_ids)
            .bind(actor_user_id)
            .bind(Uuid::now_v7())
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?
        }
    };
    let Some((star_id,)) = inserted else {
        tx.rollback().await?;
        return Ok(Err(StarDbError::NotFound));
    };
    let only = [target.kind()];
    let item = select_stars(
        &mut tx,
        workspace_id,
        actor_user_id,
        &acl,
        Some(&only),
        Some(star_id),
    )
    .await?
    .into_iter()
    .next();
    let Some(item) = item else {
        tx.rollback().await?;
        return Ok(Err(StarDbError::NotFound));
    };
    tx.commit().await?;
    Ok(Ok(item))
}

pub async fn remove_star(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    star_id: Uuid,
    kinds: Option<&[ContentKind]>,
) -> Result<Result<(), StarDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) =
        require_member_acl(&mut tx, workspace_id, actor_user_id, credential_id, true).await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let removed: Option<(Uuid,)> = sqlx::query_as(
        r#"
        DELETE FROM fvoci.stars
        WHERE workspace_id = $1 AND user_id = $2 AND id = $3
          AND (($4 AND document_id IS NOT NULL) OR ($5 AND task_id IS NOT NULL))
        RETURNING id
        "#,
    )
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(star_id)
    .bind(kind_allowed(kinds, ContentKind::Document))
    .bind(kind_allowed(kinds, ContentKind::Task))
    .fetch_optional(&mut *tx)
    .await?;
    if removed.is_none() {
        tx.rollback().await?;
        return Ok(Err(StarDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// Source `listRecent`: live documents and live, unarchived tasks the actor can
/// read, newest `updated_at` first (millisecond precision like the source).
pub async fn list_recent(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    limit: i64,
    kinds: Option<&[ContentKind]>,
) -> Result<Result<Vec<RecentItem>, StarDbError>, sqlx::Error> {
    let limit = limit.clamp(1, RECENT_MAX_LIMIT);
    let mut tx = pool.begin().await?;
    let acl = match require_member_acl(&mut tx, workspace_id, actor_user_id, credential_id, false)
        .await?
    {
        Ok(acl) => acl,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let sql = format!(
        r#"
        SELECT type, id, title, project_id, number, ua FROM (
            SELECT 'document'::text AS type, d.id, d.title, d.project_id, d.number,
                   date_trunc('milliseconds', d.updated_at) AS ua
            FROM fvoci.documents d
            WHERE d.workspace_id = $1 AND d.deleted_at IS NULL
              AND $6 AND {DOCUMENT_READ_SQL}
            UNION ALL
            SELECT 'task'::text AS type, t.id, t.title, t.project_id, t.number,
                   date_trunc('milliseconds', t.updated_at) AS ua
            FROM fvoci.tasks t
            WHERE t.workspace_id = $1 AND t.deleted_at IS NULL AND t.archived_at IS NULL
              AND $7 AND t.project_id = ANY($2)
        ) u
        ORDER BY ua DESC, id DESC
        LIMIT $5
        "#
    );
    let rows: Vec<RecentRow> = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(&acl.project_ids)
        .bind(acl.include_wiki)
        .bind(&acl.wiki_document_ids)
        .bind(limit)
        .bind(kind_allowed(kinds, ContentKind::Document))
        .bind(kind_allowed(kinds, ContentKind::Task))
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(
            |(kind, id, title, project_id, number, updated_at)| RecentItem {
                kind: parse_kind(&kind),
                id,
                title,
                project_id,
                number,
                updated_at,
            },
        )
        .collect()))
}
