//! Which block's body names which block (`body_links`): the references of the body a
//! block holds now, put down again with every write of that body
//! ([`crate::block_derived::refresh`]).
//!
//! A row says what the body says and no more: the block named may not be there, may be
//! of another workspace, may be one its writer cannot see. Nothing here asks who may
//! read what: the rows are for whoever does ask, of each reader, to start from. The
//! versions kept of a body are bodies of their own and have no rows.
//!
//! The rows stay while the block that names or the block named is in the trash, and go
//! when either is gone for good ([`forget_gone_blocks`]).

use std::collections::BTreeSet;

use fvoci_model::Block;

use crate::block_derived::Refreshing;
use crate::db::{Id, Scope, SqlFragment};
use crate::error::StoreError;

/// How many rows one statement writes or deletes: a body may name as many blocks as it
/// has nodes, and a statement has a limit of parameters.
const BATCH: usize = 300;

/// Puts down the blocks the body of `block` names now, in place of those it named:
/// only the rows that differ are written, as a body is written far more often than
/// what it names changes. Only [`crate::block_derived::refresh`] makes it, with the rest
/// of what follows from a block.
pub(crate) async fn refresh(
    scope: &mut Scope<'_>,
    block: &Block,
    _: Refreshing,
) -> Result<(), StoreError> {
    let named: BTreeSet<&str> = block
        .body
        .as_ref()
        .map(|body| body.reference_ids().into_iter().collect())
        .unwrap_or_default();
    let mut sql = scope.sql("SELECT target_id FROM body_links WHERE source_id = ");
    sql.bind(Id(&block.id));
    let held = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.id(0))
        .collect::<Result<BTreeSet<String>, _>>()?;
    let gone: Vec<&str> = held
        .iter()
        .map(String::as_str)
        .filter(|target| !named.contains(target))
        .collect();
    for batch in gone.chunks(BATCH) {
        let mut sql = scope.sql("DELETE FROM body_links WHERE source_id = ");
        sql.bind(Id(&block.id))
            .push(" AND target_id IN (")
            .bind_list(batch.iter().map(|target| Id(target)))
            .push(")");
        scope.execute(sql).await?;
    }
    let new: Vec<&str> = named
        .into_iter()
        .filter(|target| !held.contains(*target))
        .collect();
    for batch in new.chunks(BATCH) {
        let mut sql =
            scope.sql("INSERT INTO body_links (source_id, target_id, container_id) VALUES ");
        for (index, target) in batch.iter().enumerate() {
            sql.push(if index == 0 { "(" } else { ", (" })
                .bind(Id(&block.id))
                .push_bind(", ", Id(target))
                .push_bind(", ", Id(&block.container_id))
                .push(")");
        }
        scope.execute(sql).await?;
    }
    Ok(())
}

/// Forgets the rows of the blocks that are gone for good, those that named and those
/// that were named: neither on a board nor in the trash of the organization of the
/// row. A row that names a block that never was goes with them, and comes back with
/// the next write of its body.
pub(crate) async fn forget_gone_blocks(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let gone = |column: &'static str| {
        let mut frag = SqlFragment::of(&[
            "(NOT EXISTS (SELECT 1 FROM blocks b WHERE b.id = body_links.",
            column,
        ]);
        frag.push_fragment(&scope.same_organization(&[("b", "body_links")]))
            .push(") AND NOT EXISTS (SELECT 1 FROM deleted_blocks d WHERE d.id = body_links.")
            .push(column)
            .push_fragment(&scope.same_organization(&[("d", "body_links")]))
            .push("))");
        frag
    };
    let mut statement = SqlFragment::of(&["DELETE FROM body_links WHERE "]);
    statement
        .push_fragment(&gone("source_id"))
        .push(" OR ")
        .push_fragment(&gone("target_id"));
    scope.execute(scope.sql(&statement)).await?;
    Ok(())
}
