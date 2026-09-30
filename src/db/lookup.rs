use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::projects::project_member_role;
use crate::db::workspace::{membership_role, workspace_is_live};
use crate::display_id::{format_display_id, parse_display_id, ParsedDisplayId};
use crate::projects::{effective_permission, ProjectPermission};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct LookupItemRow {
    pub kind: String,
    pub id: Uuid,
    pub display_id: String,
    pub title: String,
    pub project_id: Option<Uuid>,
}

pub async fn lookup_display_id(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    display_id: &str,
    project_filter: Option<Uuid>,
) -> Result<Result<Vec<LookupItemRow>, LookupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(LookupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(LookupDbError::NotFound));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(Err(LookupDbError::NotFound));
    };
    let parsed = parse_display_id(display_id);
    if parsed.is_none() {
        tx.rollback().await?;
        return Ok(Ok(Vec::new()));
    }
    let ParsedDisplayId { prefix, number } = parsed.unwrap();
    if prefix == "WIKI" {
        if project_filter.is_some() {
            tx.rollback().await?;
            return Ok(Ok(Vec::new()));
        }
        let doc: Option<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT id, title
            FROM fvoci.documents
            WHERE workspace_id = $1 AND project_id IS NULL AND number = $2 AND deleted_at IS NULL
            "#,
        )
        .bind(workspace_id)
        .bind(number)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((id, title)) = doc else {
            tx.rollback().await?;
            return Ok(Ok(Vec::new()));
        };
        let permission = crate::db::documents::document_permission(
            &mut tx,
            workspace_id,
            actor_user_id,
            id,
            true,
        )
        .await?;
        if !permission.at_least(ProjectPermission::View) {
            tx.rollback().await?;
            return Ok(Ok(Vec::new()));
        }
        tx.commit().await?;
        return Ok(Ok(vec![LookupItemRow {
            kind: "document".to_string(),
            id,
            display_id: format_display_id("WIKI", number),
            title,
            project_id: None,
        }]));
    }

    let project: Option<(Uuid, String, String)> = sqlx::query_as(
        r#"
        SELECT id, key, visibility
        FROM fvoci.projects
        WHERE workspace_id = $1 AND key = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(&prefix)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id, project_key, visibility)) = project else {
        tx.rollback().await?;
        return Ok(Ok(Vec::new()));
    };
    // An invisible project and a filter mismatch both answer an empty list
    // (not 404), so the response does not reveal whether the key exists.
    if project_filter.is_some_and(|filter| filter != project_id) {
        tx.rollback().await?;
        return Ok(Ok(Vec::new()));
    }
    let member_role = project_member_role(&mut tx, workspace_id, project_id, actor_user_id).await?;
    let permission = effective_permission(role, &visibility, member_role);
    if !permission.at_least(ProjectPermission::View) {
        tx.rollback().await?;
        return Ok(Ok(Vec::new()));
    }

    let label = format_display_id(&project_key, number);
    let doc: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, title
        FROM fvoci.documents
        WHERE workspace_id = $1 AND project_id = $2 AND number = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(number)
    .fetch_optional(&mut *tx)
    .await?;
    let task: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, title
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND project_id = $2 AND number = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(number)
    .fetch_optional(&mut *tx)
    .await?;

    let mut items = Vec::new();
    if let Some((id, title)) = doc {
        items.push(LookupItemRow {
            kind: "document".to_string(),
            id,
            display_id: label.clone(),
            title,
            project_id: Some(project_id),
        });
    }
    if let Some((id, title)) = task {
        items.push(LookupItemRow {
            kind: "task".to_string(),
            id,
            display_id: label,
            title,
            project_id: Some(project_id),
        });
    }
    tx.commit().await?;
    Ok(Ok(items))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupDbError {
    NotFound,
    Forbidden,
}
