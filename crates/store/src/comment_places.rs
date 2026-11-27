//! Where a comment may be: on a card of its board, a page of its wiki, or a unit of
//! its course, with nothing below it (contract B1; [`BlockType::takes_comments`]).
//! Every block that is written is checked here by [`check`], and a comment is signed
//! by the agent that wrote it ([`sign`]).

use fvoci_model::{Block, BlockError, BlockType, COMMENT_AGENT_TOKEN_NAME};
use serde_json::Value;

use crate::ByAgent;

use crate::db::{Id, Scope};
use crate::error::StoreError;

/// The type of the board's block with the identifier, if the board has one.
pub(crate) async fn type_on_container(
    scope: &mut Scope<'_>,
    block_id: &str,
    container_id: &str,
) -> Result<Option<BlockType>, StoreError> {
    let mut sql = scope.sql("SELECT type FROM blocks WHERE id = ");
    sql.bind(Id(block_id))
        .push_bind(" AND container_id = ", Id(container_id));
    match scope.fetch_optional(sql).await? {
        Some(row) => Ok(Some(crate::blocks::block_type_column(&row, 0, "type")?)),
        None => Ok(None),
    }
}

/// Checks that a block that is written keeps comments where they belong: a comment is
/// on a card, a page or a unit of its container and has nothing below it. What is checked
/// is what the write changes, so a block stored before the rule is still changed in other
/// ways: a new block's parent and a moved block's new parent. `parent_type` is the type
/// of the block's parent, `None` if it has none.
pub(crate) fn check(
    block: &Block,
    parent_type: Option<&BlockType>,
    existing: Option<&Block>,
) -> Result<(), StoreError> {
    let refuse = |source| {
        Err(StoreError::InvalidBlock {
            id: block.id.clone(),
            source,
        })
    };
    let moves = existing.is_none_or(|existing| existing.parent_id != block.parent_id);
    if !moves {
        return Ok(());
    }
    let is_comment = block.block_type == BlockType::Comment;
    let on_its_place = parent_type.copied().is_some_and(BlockType::takes_comments);
    if is_comment && !on_its_place {
        return refuse(BlockError::CommentNotOnCardOrPage);
    }
    if parent_type == Some(&BlockType::Comment) {
        return refuse(BlockError::ParentIsComment);
    }
    if block.block_type == BlockType::View && !block.parent_id.is_empty() {
        return refuse(BlockError::ViewBelowBlock);
    }
    if block.block_type == BlockType::Card
        && let Some(parent) = parent_type
        && *parent != BlockType::Card
    {
        return refuse(BlockError::CardNotBelowCard);
    }
    if parent_type == Some(&BlockType::View) {
        return refuse(BlockError::ParentIsView);
    }
    if block.block_type == BlockType::Attachment && parent_type != Some(&BlockType::Card) {
        return refuse(BlockError::AttachmentNotOnCard);
    }
    if parent_type == Some(&BlockType::Attachment) {
        return refuse(BlockError::ParentIsAttachment);
    }
    Ok(())
}

/// What a block that comes back from the trash onto a parent of `parent_type` (`None`
/// for none) would break, if anything: a block stored before the rule may never have
/// kept it.
pub(crate) fn restore_refusal(
    block_type: BlockType,
    parent_type: Option<BlockType>,
) -> Option<BlockError> {
    if block_type == BlockType::Comment && !parent_type.is_some_and(BlockType::takes_comments) {
        Some(BlockError::CommentNotOnCardOrPage)
    } else if parent_type == Some(BlockType::Comment) {
        Some(BlockError::ParentIsComment)
    } else if block_type == BlockType::View && parent_type.is_some() {
        Some(BlockError::ViewBelowBlock)
    } else if parent_type == Some(BlockType::View) {
        Some(BlockError::ParentIsView)
    } else if block_type == BlockType::Attachment && parent_type != Some(BlockType::Card) {
        Some(BlockError::AttachmentNotOnCard)
    } else if parent_type == Some(BlockType::Attachment) {
        Some(BlockError::ParentIsAttachment)
    } else {
        None
    }
}

/// Says in a comment that is written which agent wrote its text
/// (`fields.agentTokenName`): the server's to say, never the writer's. Who writes a new
/// comment, or the text of one, signs it: an agent by its token's name, a person by
/// none. Any other write leaves the name as it was stored, whatever the writer sent.
pub(crate) fn sign(block: &mut Block, existing: Option<&Block>, agent: Option<ByAgent<'_>>) {
    if block.block_type != BlockType::Comment {
        return;
    }
    let text_written = existing.is_none_or(|existing| {
        crate::body_digest(existing.body.as_ref()) != crate::body_digest(block.body.as_ref())
    });
    let signed = match (existing, text_written) {
        (Some(existing), false) => existing.fields.get(COMMENT_AGENT_TOKEN_NAME).cloned(),
        _ => agent.map(|agent| Value::String(agent.token_name.to_owned())),
    };
    match signed {
        Some(name) => {
            block
                .fields
                .insert(COMMENT_AGENT_TOKEN_NAME.to_owned(), name);
        }
        None => {
            block.fields.remove(COMMENT_AGENT_TOKEN_NAME);
        }
    }
}
