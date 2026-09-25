use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::projects::seed_workflow;

pub(crate) async fn copy_project_configuration(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    source_project_id: Uuid,
    dest_project_id: Uuid,
) -> Result<(), sqlx::Error> {
    lock_projects(tx, workspace_id, source_project_id, dest_project_id).await?;

    let source_workflow: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.workflows
        WHERE workspace_id = $1 AND project_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(source_project_id)
    .fetch_optional(&mut **tx)
    .await?;

    if let Some((source_workflow_id,)) = source_workflow {
        let dest_workflow_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.workflows (id, workspace_id, project_id)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(dest_workflow_id)
        .bind(workspace_id)
        .bind(dest_project_id)
        .execute(&mut **tx)
        .await?;

        let statuses = sqlx::query_as::<_, (Uuid, String, String, String, Option<i32>)>(
            r#"
            SELECT id, name, category, sort_key, wip_limit
            FROM fvoci.statuses
            WHERE workspace_id = $1 AND workflow_id = $2
            ORDER BY sort_key COLLATE "C"
            "#,
        )
        .bind(workspace_id)
        .bind(source_workflow_id)
        .fetch_all(&mut **tx)
        .await?;

        for (_old_id, name, category, sort_key, wip_limit) in statuses {
            sqlx::query(
                r#"
                INSERT INTO fvoci.statuses (
                    id, workspace_id, project_id, workflow_id, name, category, sort_key, wip_limit
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(workspace_id)
            .bind(dest_project_id)
            .bind(dest_workflow_id)
            .bind(name)
            .bind(category)
            .bind(sort_key)
            .bind(wip_limit)
            .execute(&mut **tx)
            .await?;
        }
    } else {
        seed_workflow(tx, workspace_id, dest_project_id).await?;
    }

    let labels = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT name, color FROM fvoci.labels
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY name, id
        "#,
    )
    .bind(workspace_id)
    .bind(source_project_id)
    .fetch_all(&mut **tx)
    .await?;
    for (name, color) in labels {
        sqlx::query(
            r#"
            INSERT INTO fvoci.labels (id, workspace_id, project_id, name, color)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(dest_project_id)
        .bind(name)
        .bind(color)
        .execute(&mut **tx)
        .await?;
    }

    let milestones = sqlx::query_as::<_, (String, Option<chrono::NaiveDate>, String)>(
        r#"
        SELECT name, due_date, sort_key FROM fvoci.milestones
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(source_project_id)
    .fetch_all(&mut **tx)
    .await?;
    for (name, due_date, sort_key) in milestones {
        sqlx::query(
            r#"
            INSERT INTO fvoci.milestones (id, workspace_id, project_id, name, due_date, sort_key)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(dest_project_id)
        .bind(name)
        .bind(due_date)
        .bind(sort_key)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

async fn lock_projects(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    source_project_id: Uuid,
    dest_project_id: Uuid,
) -> Result<(), sqlx::Error> {
    let ids = [source_project_id, dest_project_id];
    sqlx::query(
        r#"
        SELECT id FROM fvoci.projects
        WHERE workspace_id = $1 AND id = ANY($2) AND deleted_at IS NULL
        ORDER BY id
        FOR NO KEY UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(())
}
