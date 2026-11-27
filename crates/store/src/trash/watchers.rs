//! Who watched the blocks in the trash: the subscriptions to a block go into the trash
//! with it, in its JSON of `deleted_blocks` (under [`KEY`]), and come back with it. A
//! subscriber whose account is gone, or erased, meanwhile comes back to nothing.

use std::collections::HashMap;

use fvoci_model::BlockType;
use serde_json::Value;

use crate::db::{Dialect, Id, Param, Scope, Sql};
use crate::error::StoreError;

/// Where a block of the trash keeps its subscriptions: no member of a block's own JSON.
const KEY: &str = "watchedBy";

/// How many identifiers one statement binds.
const BATCH: usize = 300;

/// A subscription as the trash keeps it, without its block: its own type, so that what
/// is stored does not change with the store's interface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredWatcher {
    block_type: BlockType,
    user_id: String,
    notified_at: i64,
    created_at: i64,
}

/// The subscriptions to those of `block_ids` that have any, by block, each block's in
/// the order they were made.
pub(super) async fn watchers_of(
    scope: &mut Scope<'_>,
    block_ids: &[&str],
) -> Result<HashMap<String, Vec<StoredWatcher>>, StoreError> {
    let mut watchers: HashMap<String, Vec<StoredWatcher>> = HashMap::new();
    for batch in block_ids.chunks(BATCH) {
        let mut sql = scope.sql(
            "SELECT block_id, block_type, user_id, notified_at, \
             created_at FROM subscriptions WHERE block_id IN (",
        );
        sql.bind_list(batch.iter().map(|id| Id(id)))
            .push(") ORDER BY block_id, created_at, user_id");
        for row in scope.fetch_all(sql).await? {
            watchers.entry(row.id(0)?).or_default().push(StoredWatcher {
                block_type: crate::blocks::block_type_column(&row, 1, "block_type")?,
                user_id: row.id(2)?,
                notified_at: row.int(3)?,
                created_at: row.int(4)?,
            });
        }
    }
    Ok(watchers)
}

/// The JSON of a block of the trash, `in_trash` as it is, with `watchers` beside it.
pub(super) fn with_watchers(
    in_trash: &impl serde::Serialize,
    watchers: Option<Vec<StoredWatcher>>,
) -> Result<Value, StoreError> {
    let corrupt = |error: serde_json::Error| StoreError::Corrupt {
        column: "block",
        reason: error.to_string(),
    };
    let mut value = serde_json::to_value(in_trash).map_err(corrupt)?;
    if let (Some(watchers), Some(members)) = (watchers, value.as_object_mut()) {
        members.insert(
            KEY.to_owned(),
            serde_json::to_value(watchers).map_err(corrupt)?,
        );
    }
    Ok(value)
}

/// Takes the subscriptions kept in a parsed block JSON of the trash: none for one that
/// kept none, as what went into the trash before they were kept.
pub(super) fn take_kept(block: &mut Value) -> Result<Vec<StoredWatcher>, StoreError> {
    match block.get_mut(KEY).map(Value::take) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(kept) => serde_json::from_value(kept).map_err(|error| StoreError::Corrupt {
            column: "block",
            reason: error.to_string(),
        }),
    }
}

/// Removes `user_id` from the saved watchers of the trashed blocks selected by `sql`
/// (`SELECT id, block FROM deleted_blocks WHERE ...`), pre-filtering in SQL with
/// `instr`/`strpos` so trashed blocks without `watchedBy` or `user_id` never deserialize
/// their full JSON payloads, and writing back only rows that actually named `user_id`.
async fn forget_watcher(
    scope: &mut Scope<'_>,
    mut sql: Sql,
    user_id: &str,
) -> Result<(), StoreError> {
    if !fvoci_model::is_id(user_id) {
        return Ok(());
    }
    match scope.dialect() {
        Dialect::Sqlite => {
            sql.push(" AND instr(block, 'watchedBy') > 0 AND instr(block, ")
                .bind(user_id)
                .push(") > 0");
        }
        Dialect::Postgres => {
            sql.push(" AND strpos(block::text, 'watchedBy') > 0 AND strpos(block::text, ")
                .bind(user_id)
                .push(") > 0");
        }
    }
    for row in scope.fetch_all(sql).await? {
        let mut block = row.json(1, "block")?;
        let Some(watchers) = block.get_mut(KEY).and_then(Value::as_array_mut) else {
            continue;
        };
        let before = watchers.len();
        watchers.retain(|entry| entry.get("userId").and_then(Value::as_str) != Some(user_id));
        if watchers.len() == before {
            continue;
        }
        if watchers.is_empty()
            && let Some(members) = block.as_object_mut()
        {
            members.remove(KEY);
        }
        let mut update = scope.sql("UPDATE deleted_blocks SET block = ");
        update
            .bind(Param::json(&block)?)
            .push_bind(" WHERE id = ", Id(&row.id(0)?));
        scope.execute(update).await?;
    }
    Ok(())
}

/// Forgets `user_id` among the saved watchers of blocks in the trash of `container_id`,
/// so a user removed from a board does not come back as a watcher when a block is
/// restored.
pub(crate) async fn forget_watcher_of_container(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT id, block FROM deleted_blocks WHERE container_id = ");
    sql.bind(Id(container_id));
    forget_watcher(scope, sql, user_id).await
}

/// Forgets `user_id` among the saved watchers of blocks in the trash of `workspace_id`,
/// on both live and trashed containers of that workspace.
pub(crate) async fn forget_watcher_of_workspace(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "SELECT id, block FROM deleted_blocks WHERE container_id IN (\
         SELECT id FROM containers WHERE workspace_id = ",
    );
    sql.bind(Id(workspace_id))
        .push_bind(
            " UNION SELECT id FROM deleted_containers WHERE workspace_id = ",
            Id(workspace_id),
        )
        .push(")");
    forget_watcher(scope, sql, user_id).await
}

/// Forgets `user_id` among the saved watchers of every trashed block of the scope's
/// organization (live and trashed containers alike).
pub(crate) async fn forget_watcher_of_organization(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "SELECT id, block FROM deleted_blocks WHERE container_id IN (\
         SELECT c.id FROM containers AS c JOIN workspaces AS w ON w.id = c.workspace_id \
         WHERE w.org_id = ",
    );
    sql.bind(scope.org()?)
        .push_bind(
            " UNION SELECT d.id FROM deleted_containers AS d \
             JOIN workspaces AS w ON w.id = d.workspace_id WHERE w.org_id = ",
            scope.org()?,
        )
        .push(")");
    forget_watcher(scope, sql, user_id).await
}

/// Makes again the subscriptions `block_id` kept in the trash, of the subscribers that
/// are accounts still; the block is back among the blocks. One made since stays as it is.
pub(super) async fn watch_again(
    scope: &mut Scope<'_>,
    block_id: &str,
    watchers: &[StoredWatcher],
) -> Result<(), StoreError> {
    let postgres = scope.dialect() == Dialect::Postgres;
    for watcher in watchers {
        let mut sql = scope.sql(if postgres {
            "INSERT INTO subscriptions (org_id, block_type, block_id, \
             user_id, notified_at, created_at) SELECT b.org_id, "
        } else {
            "INSERT INTO subscriptions (block_type, block_id, \
             user_id, notified_at, created_at) SELECT "
        });
        sql.bind(watcher.block_type.as_str())
            .push_bind(", b.id, u.id, ", watcher.notified_at)
            .push_bind(", ", watcher.created_at)
            .push_bind(
                " FROM blocks AS b JOIN users AS u ON u.id = ",
                Id(&watcher.user_id),
            )
            .push_bind(" WHERE u.erased_at = 0 AND b.id = ", Id(block_id))
            .push(" ON CONFLICT (block_id, user_id) DO NOTHING");
        scope.execute(sql).await?;
    }
    Ok(())
}
