//! Block row persistence, batch parent ordering, and single-roundtrip block patching.

use super::{
    BLOCK_COLUMNS, BODY_SUMMARY_COLUMNS, BODY_VERSION, Before, Block, BlockPatch, Body, BodyWrite,
    HashSet, Id, NewBodyFiles, OptId, Param, Scope, Sql, StoreError, Telling, Written,
    block_derived, body_versions, card_hierarchy, card_numbers, card_values, comment_versions,
    containers, find_container_id, get_block, insert_block_at, insert_block_inner,
    notification_noting,
};

pub(super) async fn update_existing_block_row(
    scope: &mut Scope<'_>,
    existing: &Block,
    block: &mut Block,
    user_id: &str,
    now: i64,
    how: Written<'_>,
) -> Result<(), StoreError> {
    // Every change of a block moves its `updatedAt` on, also within one millisecond
    // and when the clock steps back: a writer that read the block before this
    // change is told so (`readAt`). The stored `created_by` column is left untouched
    // in SQL while leaving the caller's in-memory `block.created_by` field as passed.
    block.created_at = existing.created_at;
    block.updated_at = now.max(existing.updated_at.saturating_add(1));
    crate::diaries::stamp_entry_body_time(block, Some(existing));
    let next = crate::card_recurrence::settle_change(scope, existing, block, how.telling).await?;
    // A block stays on its board: the update is keyed by the board too, and a block
    // of that identifier on another board is not this block.
    let mut sql = scope.sql("UPDATE blocks SET ");
    sql.push_bind("parent_id = ", OptId(&block.parent_id))
        .push_bind(", modified_by = ", OptId(&block.modified_by))
        .push_bind(", title = ", &block.title)
        .push_bind(", fields = ", Param::json(&block.fields)?)
        .push_bind(", number = ", card_numbers::number_param(block.number))
        .push_bind(", updated_at = ", block.updated_at);
    push_body(&mut sql, block.body.as_ref(), BodyColumns::Assign)?;
    sql.push_bind(" WHERE id = ", Id(&block.id))
        .push_bind(" AND container_id = ", Id(&block.container_id));
    if scope.execute(sql).await? == 0 {
        return Err(StoreError::not_found(format_args!(
            "block ID={} on board {}",
            block.id, block.container_id
        )));
    }
    block.edited_at = if comment_versions::record_change(scope, existing, block, user_id).await? {
        block.updated_at
    } else {
        existing.edited_at
    };
    // Before the files are put down: what the body before showed is still shown
    // by its version.
    body_versions::record_change(scope, existing, block, user_id, how.body_write).await?;
    block_derived::refresh(scope, block, Before::Stored(existing), how.new_files).await?;
    crate::card_events::note_changes(scope, existing, block, user_id, how.telling).await?;
    if let Some(next) = next {
        Box::pin(crate::card_recurrence::make_next(
            scope, block, next, user_id,
        ))
        .await?;
    }
    notification_noting::note_named(scope, block, Some(existing), user_id, how.telling).await
}

/// Inserts a block row with every column as `block` has it. For a new block, and for
/// one that comes back from the trash; its body's files are asked as `new_files` says.
pub(crate) async fn insert_block_row(
    scope: &mut Scope<'_>,
    block: &Block,
    new_files: NewBodyFiles,
) -> Result<(), StoreError> {
    let mut sql = scope.sql([
        "INSERT INTO blocks (",
        BLOCK_COLUMNS,
        ", ",
        BODY_SUMMARY_COLUMNS,
        ") VALUES (",
    ]);
    sql.bind(Id(&block.id))
        .push_bind(", ", OptId(&block.parent_id))
        .push_bind(", ", OptId(&block.created_by))
        .push_bind(", ", OptId(&block.modified_by))
        .push_bind(", ", block.block_type.as_str())
        .push_bind(", ", &block.title)
        .push_bind(", ", Param::json(&block.fields)?)
        .push_bind(", ", block.created_at)
        .push_bind(", ", block.updated_at)
        .push_bind(", ", Id(&block.container_id))
        .push_bind(", ", card_numbers::number_param(block.number));
    push_body(&mut sql, block.body.as_ref(), BodyColumns::List)?;
    sql.push(")");
    scope.execute(sql).await?;
    block_derived::refresh(scope, block, Before::Absent, new_files).await
}

/// How [`push_body`] writes the body's columns.
#[derive(Debug, Clone, Copy)]
pub(super) enum BodyColumns {
    /// As `, column = value` for an `UPDATE`.
    Assign,
    /// As `, value` in the order of [`BLOCK_COLUMNS`]' `body` and then
    /// [`BODY_SUMMARY_COLUMNS`], for an `INSERT`.
    List,
}

/// Appends the body and what is worked out from it (`body_version`, `body_text`,
/// `body_tasks_total`, `body_tasks_done`, `body_first_image`).
pub(super) fn push_body(
    sql: &mut Sql,
    body: Option<&Body>,
    how: BodyColumns,
) -> Result<(), StoreError> {
    let tasks = body.map_or_default(Body::tasks);
    let first_image = body.and_then(Body::first_image_file_id).map(str::to_owned);
    let values = [
        (", body = ", Param::opt_jsonb(body.map(Body::document))?),
        (
            ", body_version = ",
            Param::OptInt(body.map(|_| BODY_VERSION)),
        ),
        (", body_text = ", Param::OptText(body.map(Body::text))),
        (", body_tasks_total = ", Param::Int(i64::from(tasks.total))),
        (", body_tasks_done = ", Param::Int(i64::from(tasks.done))),
        (", body_first_image = ", Param::OptText(first_image)),
    ];
    for (assign, value) in values {
        match how {
            BodyColumns::Assign => sql.push_bind(assign, value),
            BodyColumns::List => sql.push_bind(", ", value),
        };
    }
    Ok(())
}

pub(crate) async fn insert_blocks(
    scope: &mut Scope<'_>,
    blocks: &mut [Block],
    user_id: &str,
    new_files: NewBodyFiles,
    telling: Telling,
) -> Result<(), StoreError> {
    for block in blocks.iter() {
        block
            .validate()
            .map_err(|source| StoreError::InvalidBlock {
                id: block.id.clone(),
                source,
            })?;
    }
    card_hierarchy::check_batch_cycles(blocks)?;
    containers::lock_containers(
        scope,
        blocks.iter().map(|block| block.container_id.as_str()),
    )
    .await?;
    let now = scope.now_millis();
    let mut named = Vec::new();
    for index in parents_first(blocks) {
        let block = &mut blocks[index];
        named.extend(
            insert_block_at(
                scope,
                block,
                user_id,
                Some(now),
                Written {
                    new_files,
                    body_write: BodyWrite::Whole,
                    telling,
                },
            )
            .await?,
        );
    }
    card_values::check_named_cards(scope, &named).await
}

/// The positions of the blocks in an order in which every block comes after its parent,
/// if the parent is among them. Blocks that are in a cycle of parents come last, in the
/// order they were given; they have no order that would work.
pub(crate) fn parents_first(blocks: &[Block]) -> Vec<usize> {
    let position: std::collections::HashMap<&str, usize> = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id.as_str(), index))
        .collect();
    let mut placed: HashSet<usize> = HashSet::with_capacity(blocks.len());
    let mut order = Vec::with_capacity(blocks.len());
    loop {
        let before = order.len();
        for (index, block) in blocks.iter().enumerate() {
            if placed.contains(&index) {
                continue;
            }
            let waits_for_parent = position
                .get(block.parent_id.as_str())
                .is_some_and(|parent| *parent != index && !placed.contains(parent));
            if !waits_for_parent {
                placed.insert(index);
                order.push(index);
            }
        }
        if order.len() == blocks.len() {
            return order;
        }
        if order.len() == before {
            order.extend((0..blocks.len()).filter(|index| !placed.contains(index)));
            return order;
        }
    }
}

/// Applies a patch to a block and returns the updated block; the versions of a card's
/// body are kept as `body_write` says, and the people the patched block newly names told
/// as `telling` says.
pub(crate) async fn patch_block(
    scope: &mut Scope<'_>,
    block_id: &str,
    patch: &BlockPatch,
    user_id: &str,
    body_write: BodyWrite<'_>,
    telling: Telling,
) -> Result<Block, StoreError> {
    let container_id = find_container_id(scope, block_id)
        .await?
        .ok_or_else(|| StoreError::not_found(format_args!("block ID={block_id}")))?;
    let Some(kind) = containers::lock_container_of_kind(scope, &container_id).await? else {
        return Err(StoreError::not_found(format_args!(
            "board ID={container_id} of block {block_id}"
        )));
    };
    let existing = get_block(scope, block_id).await?;
    let (min_read_at, stored_updated_at) = crate::diaries::read_at_window(&existing);
    let mut block = existing.clone();
    block.drop_malformed_fields();
    patch.apply(&mut block);
    block
        .validate()
        .map_err(|source| StoreError::InvalidBlock {
            id: block.id.clone(),
            source,
        })?;
    if let Some(read_at) = patch.read_at
        && !(min_read_at..=stored_updated_at).contains(&read_at)
    {
        return Err(StoreError::Stale {
            id: block.id,
            read_at,
            updated_at: stored_updated_at,
        });
    }
    let how = Written {
        new_files: NewBodyFiles::Recorded,
        body_write,
        telling,
    };
    let named = insert_block_inner(
        scope,
        &mut block,
        user_id,
        None,
        how,
        Some((kind, existing)),
    )
    .await?;
    card_values::check_named_cards(scope, &named).await?;
    Ok(block)
}
