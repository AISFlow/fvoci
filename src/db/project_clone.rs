use std::collections::HashMap;

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::collections::{parse_query_config, DateBy, GroupBy};
use crate::db::projects::seed_workflow;
use crate::tasks::list_query::{CustomOperator, CustomValue, SortField, ViewQuery};

/// Old → new ids of everything a copied view may reference (source `Catalog`).
#[derive(Debug, Default)]
pub(crate) struct CloneCatalog {
    pub statuses: HashMap<Uuid, Uuid>,
    pub labels: HashMap<Uuid, Uuid>,
    pub milestones: HashMap<Uuid, Uuid>,
    pub fields: HashMap<Uuid, Uuid>,
    /// Per option-type field (source `optionsByField`): old option → new option.
    pub options_by_field: HashMap<Uuid, HashMap<Uuid, Uuid>>,
}

impl CloneCatalog {
    /// `None` when the view references something that was not copied (source
    /// `uncopyable`: the clone is rejected rather than saving a broken view).
    pub fn remap_view_query(&self, query: &ViewQuery) -> Option<ViewQuery> {
        let mut out = query.clone();
        let f = &mut out.filters;
        if let Some(id) = f.status_id {
            f.status_id = Some(*self.statuses.get(&id)?);
        }
        if let Some(id) = f.label_id {
            f.label_id = Some(*self.labels.get(&id)?);
        }
        if let Some(id) = f.milestone_id {
            f.milestone_id = Some(*self.milestones.get(&id)?);
        }
        for item in &mut f.custom {
            let old_field = item.field_id;
            item.field_id = *self.fields.get(&old_field)?;
            // Source `remapCustom`: on an option field anything but `empty` must
            // be a copied option of that same field, otherwise uncopyable.
            let Some(options) = self.options_by_field.get(&old_field) else {
                continue;
            };
            match &mut item.operator {
                CustomOperator::Empty => {}
                CustomOperator::Equals(CustomValue::Text(raw)) => {
                    let option = Uuid::parse_str(raw).ok()?;
                    *raw = options.get(&option)?.to_string();
                }
                CustomOperator::Equals(_) => return None,
            }
        }
        for entry in &mut out.sort {
            if let SortField::Field(id) = entry.field {
                entry.field = SortField::Field(*self.fields.get(&id)?);
            }
        }
        Some(out)
    }

    pub fn remap_collection_config(&self, config: &Value) -> Option<Value> {
        let mut parsed = parse_query_config(config).ok()?;
        parsed.query = self.remap_view_query(&parsed.query)?;
        parsed.group_by = match parsed.group_by {
            Some(GroupBy::Field(id)) => self.fields.get(&id).map(|id| GroupBy::Field(*id)),
            other => other,
        };
        parsed.date_by = match parsed.date_by {
            Some(DateBy::Field(id)) => self.fields.get(&id).map(|id| DateBy::Field(*id)),
            other => other,
        };
        Some(parsed.to_json())
    }
}

/// Copies workflow, labels, milestones and the task collection (fields,
/// options, shared views). `Ok(false)`: a shared view could not be remapped.
pub(crate) async fn copy_project_configuration(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    source_project_id: Uuid,
    dest_project_id: Uuid,
    actor_user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut catalog = CloneCatalog::default();
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

        for (old_id, name, category, sort_key, wip_limit) in statuses {
            let new_id = Uuid::now_v7();
            catalog.statuses.insert(old_id, new_id);
            sqlx::query(
                r#"
                INSERT INTO fvoci.statuses (
                    id, workspace_id, project_id, workflow_id, name, category, sort_key, wip_limit
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(new_id)
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

    let labels = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT id, name, color FROM fvoci.labels
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY name, id
        "#,
    )
    .bind(workspace_id)
    .bind(source_project_id)
    .fetch_all(&mut **tx)
    .await?;
    for (old_id, name, color) in labels {
        let new_id = Uuid::now_v7();
        catalog.labels.insert(old_id, new_id);
        sqlx::query(
            r#"
            INSERT INTO fvoci.labels (id, workspace_id, project_id, name, color)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(new_id)
        .bind(workspace_id)
        .bind(dest_project_id)
        .bind(name)
        .bind(color)
        .execute(&mut **tx)
        .await?;
    }

    let milestones = sqlx::query_as::<_, (Uuid, String, Option<chrono::NaiveDate>, String)>(
        r#"
        SELECT id, name, due_date, sort_key FROM fvoci.milestones
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(source_project_id)
    .fetch_all(&mut **tx)
    .await?;
    for (old_id, name, due_date, sort_key) in milestones {
        let new_id = Uuid::now_v7();
        catalog.milestones.insert(old_id, new_id);
        sqlx::query(
            r#"
            INSERT INTO fvoci.milestones (id, workspace_id, project_id, name, due_date, sort_key)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(new_id)
        .bind(workspace_id)
        .bind(dest_project_id)
        .bind(name)
        .bind(due_date)
        .bind(sort_key)
        .execute(&mut **tx)
        .await?;
    }

    Ok(crate::db::collections::copy_task_collection(
        tx,
        workspace_id,
        source_project_id,
        dest_project_id,
        actor_user_id,
        &mut catalog,
    )
    .await?
    .is_ok())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::list_query::CustomFilter;

    fn query_with(filters: Vec<CustomFilter>) -> ViewQuery {
        let mut query = ViewQuery::default();
        query.filters.custom = filters;
        query
    }

    #[test]
    fn option_filters_must_name_a_copied_option_of_the_same_field() {
        let (select, other_select, text) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let (kept, deleted, other_option) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let mut catalog = CloneCatalog::default();
        for field in [select, other_select, text] {
            catalog.fields.insert(field, Uuid::now_v7());
        }
        let new_kept = Uuid::now_v7();
        catalog
            .options_by_field
            .insert(select, HashMap::from([(kept, new_kept)]));
        catalog.options_by_field.insert(
            other_select,
            HashMap::from([(other_option, Uuid::now_v7())]),
        );
        let equals = |field, value: String| CustomFilter {
            field_id: field,
            operator: CustomOperator::Equals(CustomValue::Text(value)),
        };

        let remapped = catalog
            .remap_view_query(&query_with(vec![
                equals(select, kept.to_string()),
                CustomFilter {
                    field_id: select,
                    operator: CustomOperator::Empty,
                },
                equals(text, deleted.to_string()),
            ]))
            .expect("copyable");
        assert_eq!(
            remapped.filters.custom[0].operator,
            CustomOperator::Equals(CustomValue::Text(new_kept.to_string()))
        );
        assert_eq!(remapped.filters.custom[1].operator, CustomOperator::Empty);
        // Text fields keep their literal value, even if it looks like an id.
        assert_eq!(
            remapped.filters.custom[2].operator,
            CustomOperator::Equals(CustomValue::Text(deleted.to_string()))
        );
        // A deleted option, another field's option, or a non-text value is uncopyable.
        for filter in [
            equals(select, deleted.to_string()),
            equals(select, other_option.to_string()),
            equals(select, "not-an-id".to_string()),
            CustomFilter {
                field_id: select,
                operator: CustomOperator::Equals(CustomValue::Bool(true)),
            },
        ] {
            assert!(catalog
                .remap_view_query(&query_with(vec![filter]))
                .is_none());
        }
    }
}
