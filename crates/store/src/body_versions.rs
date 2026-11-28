//! The versions kept of the bodies of cards (`body_versions`), and who wrote the body
//! up to each (`body_version_editors`).
//!
//! The server keeps a card's body as a version when it changed since the newest one
//! ([`Store::keep_body_version`]): which body that is, and when, is the server's to
//! say (`crates/server/src/collab`); the store tells a body that is kept already by its
//! digest, numbers the versions, and keeps the newest within both the count and byte
//! budgets ([`Store::set_body_versions_kept`], [`Store::set_body_versions_max_bytes`]).
//! A card in the trash keeps its rows; they go when the card is gone for good
//! ([`forget_gone_blocks`]). The files the kept versions show are not collected
//! ([`crate::file_refs`]).
//!
//! Each version says how it came to be kept ([`BodyVersionCause`]): by an editor who
//! asked for it ([`Store::keep_body_version_by_hand`]), as the body a version was gone
//! back to ([`Store::restore_body`]), or by the server on its own.
//!
//! A page's body is kept as a card's is: "card" here is any block whose body has a room
//! ([`BlockType::has_room`]).

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use fvoci_model::{Block, BlockPatch, Body, BodyVersionCause};
use serde_json::Value;

use crate::Store;
use crate::block_derived::{self, Before};
use crate::body_digest::body_digest;
use crate::db::{FromRow, Id, OptId, Param, Row, Scope, SqlFragment, VersionsKept, to_json_text};
use crate::error::StoreError;
use crate::file_refs::NewBodyFiles;
use crate::notification_noting::Telling;
use crate::{blocks, containers};

/// How many versions of a card's body the store keeps unless told otherwise.
pub const DEFAULT_BODY_VERSIONS_KEPT: u32 = 50;

/// Maximum total uncompressed serialized JSON bytes of kept body versions per card
/// unless told otherwise: 64 MiB (8 versions at the 8 MiB [`fvoci_model::BODY_MAX_BYTES`]
/// limit; bounds per-card disk growth even if the per-body limit increases later).
pub const DEFAULT_BODY_VERSIONS_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The version of [`body_version_editors`] rows for who wrote since the newest version.
pub(crate) const PENDING: i64 = 0;

/// The SQL condition that `column`, the type of a block, is one whose body is kept
/// ([`fvoci_model::BlockType::WITH_ROOM`]).
pub(crate) fn with_room_sql(column: &'static str) -> SqlFragment {
    SqlFragment::of(&[column, " IN ('card', 'page', 'entry', 'unit')"])
}

/// A version kept of a card's body, without the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptVersion {
    /// Its number among the card's, from 1; versions no longer kept leave the lowest
    /// numbers out.
    pub version: i64,
    /// When it was kept, in milliseconds since the epoch.
    pub kept_at: i64,
    /// Who wrote the body up to it since the version before, in the order of their
    /// identifiers.
    pub editors: Vec<String>,
    /// The digest of its body, as the server takes it.
    pub body_digest: Vec<u8>,
    /// How it came to be kept.
    pub cause: BodyVersionCause,
    /// The name of the token an agent wrote it through ([`BodyVersionCause::Agent`]).
    pub agent_token_name: Option<String>,
}

/// An agent that writes through a personal API token of its user, as the version it
/// leaves names it and its notifications tell of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByAgent<'a> {
    /// The token the agent writes through.
    pub token_id: &'a str,
    /// The name of the token when the agent wrote, as the version and the
    /// notifications show it.
    pub token_name: &'a str,
    /// Whether the token's owner is told of what the agent does, as anybody else would
    /// be: of a comment on what they watch, of being named or given a card.
    pub notify_self: bool,
}

/// [`ByAgent`] as a handle of the store keeps it ([`Store::by_agent`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentOfHandle {
    token_id: String,
    token_name: String,
    notify_self: bool,
}

impl AgentOfHandle {
    pub(crate) fn of(agent: ByAgent<'_>) -> Self {
        Self {
            token_id: agent.token_id.to_owned(),
            token_name: agent.token_name.to_owned(),
            notify_self: agent.notify_self,
        }
    }

    pub(crate) fn by(&self) -> ByAgent<'_> {
        ByAgent {
            token_id: &self.token_id,
            token_name: &self.token_name,
            notify_self: self.notify_self,
        }
    }
}

/// What became of a card's body an editor asked to keep as a version
/// ([`Store::keep_body_version_by_hand`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByHand {
    /// Kept as the version of this number.
    Kept(i64),
    /// The body is the card's newest version already, of this number; `None` for no
    /// body of a card that has no version. Nothing is kept, and nobody is left as
    /// having written since, as when the server finds it so.
    Unchanged(Option<i64>),
    /// A version was kept by hand at or after the time asked for: the newest such.
    /// Nothing is kept, and who wrote since stays put down for the next version.
    Recent(i64),
}

/// A card's body to keep as a version, if it is not the card's newest one.
#[derive(Debug, Clone, Copy)]
struct BodyToKeep<'a> {
    /// The card as it is stored: its body is the version.
    block: &'a Block,
    /// Who wrote it whole; `None` for a body its editors typed, who are known already
    /// ([`Store::append_body_updates`]).
    written_by: Option<&'a str>,
    /// How it comes to be kept.
    cause: BodyVersionCause,
    /// The agent that wrote it, for a version of [`BodyVersionCause::Agent`].
    agent: Option<ByAgent<'a>>,
}

impl Store {
    /// Sets how many versions of a card's body are kept, the newest. At least one.
    /// Counts for the versions kept after.
    pub fn set_body_versions_kept(&self, kept: u32) {
        self.inner
            .body_versions_kept
            .store(kept.max(1), Ordering::Relaxed);
    }

    /// Sets the byte budget for all kept versions of a card's body combined, in
    /// uncompressed serialized JSON bytes. At least one byte. Counts for the versions
    /// kept after; the newest version is always kept even if it alone exceeds the budget.
    pub fn set_body_versions_max_bytes(&self, max_bytes: u64) {
        self.inner
            .body_versions_max_bytes
            .store(max_bytes.max(1), Ordering::Relaxed);
    }

    /// Keeps the body a card holds as a version, and gives the number of the new
    /// version; `None` for a body that is the card's newest version already, for no
    /// body of a card that has no version, and for a card that is not there.
    ///
    /// The new version names who wrote the body since the one before: the users whose
    /// editors sent updates ([`Store::append_body_updates`]), and nobody is left as
    /// having written since. One an agent wrote in since is the agent's
    /// ([`BodyVersionCause::Agent`]), as if kept as soon as it wrote. If nobody is known to have (a body from before versions
    /// were kept, or of an import), it names nobody and is as of the card's last
    /// change. Versions past those kept go, the oldest first, and the files only they
    /// showed are shown no more.
    ///
    /// The card is read with its board held, as a write of the card holds it: the
    /// body kept is one the card had, whatever is written meanwhile.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails and [`StoreError::Corrupt`]
    /// if a stored block column or version column cannot be decoded.
    pub async fn keep_body_version(&self, card_id: &str) -> Result<Option<i64>, StoreError> {
        self.transaction(async |scope| keep_held(scope, card_id).await)
            .await
    }

    /// Keeps the body a card holds as a version an editor asked for
    /// ([`BodyVersionCause::Manual`]), unless it is the card's newest version already
    /// or a version was kept by hand at or after `since` (in milliseconds since the
    /// epoch); `None` for a card that is not there. As [`Store::keep_body_version`]
    /// otherwise, but as of now whoever is named: who asked is not named for asking.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails and [`StoreError::Corrupt`]
    /// if a stored block column or version column cannot be decoded.
    pub async fn keep_body_version_by_hand(
        &self,
        card_id: &str,
        since: i64,
    ) -> Result<Option<ByHand>, StoreError> {
        self.transaction(async |scope| {
            let Some(card) = held_card(scope, card_id).await? else {
                return Ok(None);
            };
            let newest = newest_version(scope, &card.id).await?;
            if !is_kept_already(&card, newest.as_ref()) {
                let mut sql = scope.sql("SELECT version FROM body_versions WHERE block_id = ");
                sql.bind(Id(&card.id))
                    .push_bind(" AND cause = ", BodyVersionCause::Manual.as_str())
                    .push_bind(" AND kept_at >= ", since)
                    .push(" ORDER BY version DESC LIMIT 1");
                if let Some(row) = scope.fetch_optional(sql).await? {
                    return Ok(Some(ByHand::Recent(row.int(0)?)));
                }
            }
            let body = BodyToKeep {
                block: &card,
                written_by: None,
                cause: BodyVersionCause::Manual,
                agent: None,
            };
            let kept = keep_and_refresh(scope, &card, &body).await?;
            Ok(Some(match kept {
                Some(version) => ByHand::Kept(version),
                None => ByHand::Unchanged(newest.map(|(version, _)| version)),
            }))
        })
        .await
    }

    /// Keeps the body a card holds as the version an agent leaves, by `user_id` through
    /// the token `agent` ([`BodyVersionCause::Agent`]), and gives its number; `None` as
    /// for [`Store::keep_body_version`]. Who else wrote the body since the version
    /// before is named with them.
    ///
    /// # Errors
    ///
    /// As [`Store::keep_body_version`].
    pub async fn keep_agent_version(
        &self,
        card_id: &str,
        user_id: &str,
        agent: ByAgent<'_>,
    ) -> Result<Option<i64>, StoreError> {
        self.transaction(async |scope| {
            let Some(card) = held_card(scope, card_id).await? else {
                return Ok(None);
            };
            let body = BodyToKeep {
                block: &card,
                written_by: Some(user_id),
                cause: BodyVersionCause::Agent,
                agent: Some(agent),
            };
            keep_and_refresh(scope, &card, &body).await
        })
        .await
    }

    /// Applies `patch` to a block as [`Store::patch_block`] does, written by an agent
    /// through a personal API token of `user_id`: a body it writes whole is kept as
    /// the agent's version ([`BodyVersionCause::Agent`]) after the body before.
    ///
    /// # Errors
    ///
    /// As [`Store::patch_block`].
    pub async fn patch_block_by_agent(
        &self,
        block_id: &str,
        patch: &BlockPatch,
        user_id: &str,
        agent: ByAgent<'_>,
    ) -> Result<Block, StoreError> {
        self.transaction(async |scope| {
            let telling = Telling::of(scope);
            blocks::patch_block(
                scope,
                block_id,
                patch,
                user_id,
                BodyWrite::ByAgent(agent),
                telling,
            )
            .await
        })
        .await
    }

    /// Writes the body of a version to a card again, as [`Store::patch_block`] writes
    /// a body by `user_id`: the body written is kept as the card's newest version
    /// ([`BodyVersionCause::Restored`]) after the body it is written over. Whom it
    /// names is told nothing: the words of a version are whose wrote them then, and
    /// not of who brings them back.
    ///
    /// # Errors
    ///
    /// As [`Store::patch_block`].
    pub async fn restore_body(
        &self,
        card_id: &str,
        patch: &BlockPatch,
        user_id: &str,
    ) -> Result<Block, StoreError> {
        self.transaction(async |scope| {
            blocks::patch_block(
                scope,
                card_id,
                patch,
                user_id,
                BodyWrite::Restored,
                Telling::Nobody,
            )
            .await
        })
        .await
    }

    /// How many versions and body bytes are kept, for a scope that begins.
    pub(crate) fn versions_kept(&self) -> VersionsKept {
        VersionsKept {
            of_comments: u32::try_from(self.comment_versions_kept()).unwrap_or(u32::MAX),
            of_bodies: self.inner.body_versions_kept.load(Ordering::Relaxed),
            body_max_bytes: self.inner.body_versions_max_bytes.load(Ordering::Relaxed),
        }
    }

    /// The versions kept of a card's body, the newest first, without their bodies.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails and [`StoreError::Corrupt`] if
    /// a stored version cause is unknown.
    pub async fn body_versions(&self, block_id: &str) -> Result<Vec<KeptVersion>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT version, kept_at, body_digest, cause, actor_token_name FROM body_versions \
                 WHERE block_id = ",
            );
            sql.bind(Id(block_id)).push(" ORDER BY version DESC");
            let mut versions: Vec<KeptVersion> = scope.all(sql).await?;
            let mut sql =
                scope.sql("SELECT version, user_id FROM body_version_editors WHERE block_id = ");
            sql.bind(Id(block_id))
                .push_bind(" AND version > ", PENDING)
                .push(" ORDER BY user_id");
            for row in scope.fetch_all(sql).await? {
                let (version, user_id) = (row.int(0)?, row.id(1)?);
                if let Some(kept) = versions.iter_mut().find(|kept| kept.version == version) {
                    kept.editors.push(user_id);
                }
            }
            Ok(versions)
        })
        .await
    }

    /// A version kept of a card's body, with the body; `None` if the card has no such
    /// version (any more).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails and [`StoreError::Corrupt`] if
    /// a stored version cause or body JSON cannot be decoded.
    pub async fn body_version(
        &self,
        block_id: &str,
        version: i64,
    ) -> Result<Option<(KeptVersion, Option<Body>)>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT version, kept_at, body_digest, cause, actor_token_name, body \
                 FROM body_versions WHERE block_id = ",
            );
            sql.bind(Id(block_id)).push_bind(" AND version = ", version);
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(None);
            };
            let mut kept = KeptVersion::from_row(&row)?;
            let body = match row.json(5, "body")? {
                Value::Null => None,
                document => Some(Body::from_stored(document)),
            };
            let mut sql = scope.sql("SELECT user_id FROM body_version_editors WHERE block_id = ");
            sql.bind(Id(block_id))
                .push_bind(" AND version = ", version)
                .push(" ORDER BY user_id");
            for row in scope.fetch_all(sql).await? {
                kept.editors.push(row.id(0)?);
            }
            Ok(Some((kept, body)))
        })
        .await
    }
}

/// A version as a row of `version, kept_at, body_digest, cause, actor_token_name` has
/// it, without who wrote it.
impl FromRow for KeptVersion {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        let cause = row.text(3)?;
        Ok(KeptVersion {
            version: row.int(0)?,
            kept_at: row.int(1)?,
            body_digest: row.bytes(2)?,
            editors: Vec::new(),
            agent_token_name: row.opt_text(4)?,
            cause: cause
                .parse()
                .map_err(
                    |error: fvoci_model::ParseBodyVersionCauseError| StoreError::Corrupt {
                        column: "cause",
                        reason: error.to_string(),
                    },
                )?,
        })
    }
}

/// What becomes of the body a card holds before updates of its document are logged
/// ([`Store::append_body_updates`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyBefore {
    /// Kept as a version first, if it is not the card's newest, within the write of
    /// the log: updates that are not logged keep nothing. For a card that holds all
    /// its document took before.
    Keep,
    /// Left as it is: a version already, or less than the document took.
    Leave,
}

/// Keeps the body a card holds ([`Store::keep_body_version`]), read with its board
/// held.
pub(crate) async fn keep_held(
    scope: &mut Scope<'_>,
    card_id: &str,
) -> Result<Option<i64>, StoreError> {
    let Some(card) = held_card(scope, card_id).await? else {
        return Ok(None);
    };
    let body = BodyToKeep {
        block: &card,
        written_by: None,
        cause: BodyVersionCause::Auto,
        agent: None,
    };
    keep_and_refresh(scope, &card, &body).await
}

/// A card whose body has a room, read with its board held; `None` if it is not there.
async fn held_card(scope: &mut Scope<'_>, card_id: &str) -> Result<Option<Block>, StoreError> {
    let Some(found) = blocks::find_block(scope, card_id).await? else {
        return Ok(None);
    };
    // Read again once its board is held: what puts down the files a card shows holds
    // the board, and so does what writes its body.
    if !containers::lock_container(scope, &found.container_id).await? {
        return Ok(None);
    }
    Ok(blocks::find_block(scope, card_id)
        .await?
        .filter(|block| block.block_type.has_room()))
}

/// Keeps the body of a card read with its board held ([`keep`]), and puts down the
/// files it shows again if versions past those kept went; the number of the new
/// version, if one was kept.
async fn keep_and_refresh(
    scope: &mut Scope<'_>,
    card: &Block,
    body: &BodyToKeep<'_>,
) -> Result<Option<i64>, StoreError> {
    let kept = keep(scope, body).await?;
    if kept.forgot_some {
        // The files the card shows are put down again with those of the versions
        // that are left.
        block_derived::refresh(scope, card, Before::Unchanged, NewBodyFiles::Unchecked).await?;
    }
    Ok(kept.version)
}

/// What a write that changes the body of a card is, for the versions kept of that body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyWrite<'a> {
    /// A body written whole by its writer: the body before and the body after are
    /// kept as versions within the write ([`record_change`]).
    Whole,
    /// The body of a version written whole again by its writer ([`Store::restore_body`]):
    /// kept as [`BodyWrite::Whole`] keeps a body, the body after as
    /// [`BodyVersionCause::Restored`].
    Restored,
    /// A body written whole by an agent through a personal API token of its writer
    /// ([`Store::patch_block_by_agent`]): kept as [`BodyWrite::Whole`] keeps a body, the
    /// body after as [`BodyVersionCause::Agent`], named by the token.
    ByAgent(ByAgent<'a>),
    /// The body of the card's document as its editors typed it so far
    /// ([`Store::project_body`]): no version of its own; the server keeps one when it
    /// is due ([`Store::keep_body_version`]).
    Projected,
}

impl BodyWrite<'_> {
    /// How the body after a write is kept as a version within it ([`record_change`]);
    /// `None` for a write that keeps none.
    pub(crate) const fn kept_after(self) -> Option<BodyVersionCause> {
        match self {
            Self::Whole => Some(BodyVersionCause::Auto),
            Self::Restored => Some(BodyVersionCause::Restored),
            Self::ByAgent(_) => Some(BodyVersionCause::Agent),
            Self::Projected => None,
        }
    }
}

/// What became of a body that was to be kept.
#[derive(Debug, Clone, Copy)]
struct Kept {
    /// The number of the new version; `None` if the body was kept already.
    version: Option<i64>,
    /// Whether versions past those kept went: the files the card shows are to be put
    /// down again.
    forgot_some: bool,
}

/// Keeps the versions a write of a card's body whole brings, within that write: the
/// body before, if it is not the card's newest version yet (by its editors, if any
/// typed it), and the body after, by `user_id`, as `write` says
/// ([`BodyWrite::kept_after`]). Nothing for a write that keeps no version, that leaves
/// the body as it was, and for a block whose body has no room. The caller puts down
/// the files the card shows after.
pub(crate) async fn record_change(
    scope: &mut Scope<'_>,
    before: &Block,
    after: &Block,
    user_id: &str,
    write: BodyWrite<'_>,
) -> Result<(), StoreError> {
    let Some(cause) = write.kept_after() else {
        return Ok(());
    };
    if !after.block_type.has_room()
        || body_digest(before.body.as_ref()) == body_digest(after.body.as_ref())
    {
        return Ok(());
    }
    if before.block_type.has_room() {
        let before = BodyToKeep {
            block: before,
            written_by: None,
            cause: BodyVersionCause::Auto,
            agent: None,
        };
        keep(scope, &before).await?;
    }
    let after = BodyToKeep {
        block: after,
        written_by: Some(user_id),
        cause,
        agent: match write {
            BodyWrite::ByAgent(agent) => Some(agent),
            _ => None,
        },
    };
    keep(scope, &after).await?;
    Ok(())
}

/// Records the initial body version of a newly duplicated room-bearing block inside the
/// duplication transaction, when the copied block carries a body.
pub(crate) async fn keep_copied_body(
    scope: &mut Scope<'_>,
    block: &Block,
    user_id: &str,
) -> Result<(), StoreError> {
    if !block.block_type.has_room() || block.body.is_none() {
        return Ok(());
    }
    let copied = BodyToKeep {
        block,
        written_by: Some(user_id),
        cause: BodyVersionCause::Auto,
        agent: None,
    };
    keep_and_refresh(scope, block, &copied).await?;
    Ok(())
}

/// Keeps one body ([`Store::keep_body_version`]) and forgets the versions past those
/// kept.
async fn keep(scope: &mut Scope<'_>, body: &BodyToKeep<'_>) -> Result<Kept, StoreError> {
    let block = body.block;
    let digest = body_digest(block.body.as_ref());
    let newest = newest_version(scope, &block.id).await?;
    let kept_already = is_kept_already(block, newest.as_ref());
    let mut sql = scope.sql("DELETE FROM body_version_editors WHERE block_id = ");
    sql.bind(Id(&block.id))
        .push_bind(" AND version = ", PENDING)
        .push(" RETURNING user_id");
    let mut editors: BTreeSet<String> = scope.all(sql).await?;
    let pending_agent = take_pending_agent(scope, &block.id).await?;
    if kept_already {
        return Ok(Kept {
            version: None,
            forgot_some: false,
        });
    }
    // A writer without an identifier (the server itself) is nobody to name.
    if let Some(written_by) = body.written_by.filter(|by| !by.is_empty()) {
        editors.insert(written_by.to_owned());
    }
    // Nobody is known to have written it: the body is as old as the card's last
    // change at the latest, and who made that change may have changed something else.
    // One asked for is kept when it was asked for.
    let kept_at = if editors.is_empty() && body.cause != BodyVersionCause::Manual {
        block.updated_at
    } else {
        scope.now_millis()
    };
    // A body an agent wrote in since the newest version is the agent's, whoever wrote
    // it to the card and whoever else typed in it: as the version kept as soon as the
    // agent wrote would have been.
    let (cause, agent) = match (&pending_agent, body.agent) {
        (Some((token_id, token_name)), None) if body.cause == BodyVersionCause::Auto => (
            BodyVersionCause::Agent,
            Some(ByAgent {
                token_id,
                token_name,
                notify_self: false,
            }),
        ),
        _ => (body.cause, body.agent),
    };
    let version = newest.as_ref().map_or(1, |(version, _)| version + 1);
    let body_json = block
        .body
        .as_ref()
        .map(|doc| to_json_text(doc.document()))
        .transpose()?;
    let body_bytes = body_json
        .as_deref()
        .map_or(0_i64, |json| i64::try_from(json.len()).unwrap_or(i64::MAX));
    let files: Vec<&str> = block.body.as_ref().map_or_default(Body::file_ids);
    let mut sql = scope.sql(
        "INSERT INTO body_versions (block_id, version, body, body_bytes, body_digest, \
         files, kept_at, cause, actor_token_id, actor_token_name) VALUES (",
    );
    sql.bind(Id(&block.id))
        .push_bind(", ", version)
        .push_bind(", ", Param::OptJsonb(body_json))
        .push_bind(", ", body_bytes)
        .push_bind(", ", Param::Bytes(digest))
        .push_bind(", ", to_json_text(&files)?)
        .push_bind(", ", kept_at)
        .push_bind(", ", cause.as_str())
        .push_bind(", ", OptId(agent.map_or("", |agent| agent.token_id)))
        .push_bind(
            ", ",
            Param::OptText(agent.map(|agent| agent.token_name.to_owned())),
        )
        .push(")");
    scope.execute(sql).await?;
    name_editors(scope, &block.id, version, &editors).await?;
    let through = version.saturating_sub(scope.body_versions_kept());
    let forgot_by_count = forget_past(scope, &block.id, through).await?;
    let forgot_by_bytes = if newest.is_some() && scope.body_versions_kept() > 1 {
        forget_over_budget(scope, &block.id, version).await?
    } else {
        false
    };
    Ok(Kept {
        version: Some(version),
        forgot_some: forgot_by_count || forgot_by_bytes,
    })
}

/// Puts down `editors` as those who wrote version `version` of a card's body.
async fn name_editors(
    scope: &mut Scope<'_>,
    block_id: &str,
    version: i64,
    editors: &BTreeSet<String>,
) -> Result<(), StoreError> {
    if editors.is_empty() {
        return Ok(());
    }
    let mut sql =
        scope.sql("INSERT INTO body_version_editors (block_id, version, user_id) VALUES ");
    for (index, user_id) in editors.iter().enumerate() {
        sql.push(if index == 0 { "(" } else { ", (" })
            .bind(Id(block_id))
            .push_bind(", ", version)
            .push_bind(", ", Id(user_id))
            .push(")");
    }
    scope.execute(sql).await?;
    Ok(())
}

/// The agent put down as having written a card's body since its newest version
/// ([`Store::append_body_updates`]), by its token and the token's name, if one is; it
/// is put down no more.
async fn take_pending_agent(
    scope: &mut Scope<'_>,
    block_id: &str,
) -> Result<Option<(String, String)>, StoreError> {
    let mut sql =
        scope.sql("SELECT agent_token_id, agent_token_name FROM body_docs WHERE block_id = ");
    sql.bind(Id(block_id))
        .push(" AND agent_token_id IS NOT NULL");
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(None);
    };
    let pending = (row.id(0)?, row.opt_text(1)?.unwrap_or_default());
    let mut sql = scope.sql(
        "UPDATE body_docs SET agent_token_id = NULL, agent_token_name = NULL WHERE block_id = ",
    );
    sql.bind(Id(block_id));
    scope.execute(sql).await?;
    Ok(Some(pending))
}

/// The number and the body digest of a card's newest version, if it has one.
async fn newest_version(
    scope: &mut Scope<'_>,
    block_id: &str,
) -> Result<Option<(i64, Vec<u8>)>, StoreError> {
    let mut sql = scope.sql("SELECT version, body_digest FROM body_versions WHERE block_id = ");
    sql.bind(Id(block_id))
        .push(" ORDER BY version DESC LIMIT 1");
    match scope.fetch_optional(sql).await? {
        Some(row) => Ok(Some((row.int(0)?, row.bytes(1)?))),
        None => Ok(None),
    }
}

/// Whether the body a block holds is kept already: it is that of its newest version,
/// or it is no body and the block has no version. No body is no first version: there
/// is nothing in it to go back to.
fn is_kept_already(block: &Block, newest: Option<&(i64, Vec<u8>)>) -> bool {
    match newest {
        Some((_, digest)) => *digest == body_digest(block.body.as_ref()),
        None => block.body.is_none(),
    }
}

/// Forgets the versions of a card up to `through`; whether there were any. The files
/// only they showed are shown no more once the caller puts the card's files down again.
async fn forget_past(
    scope: &mut Scope<'_>,
    block_id: &str,
    through: i64,
) -> Result<bool, StoreError> {
    if through < 1 {
        return Ok(false);
    }
    let mut sql = scope.sql("DELETE FROM body_versions WHERE block_id = ");
    sql.bind(Id(block_id))
        .push_bind(" AND version <= ", through);
    if scope.execute(sql).await? == 0 {
        return Ok(false);
    }
    let mut sql = scope.sql("DELETE FROM body_version_editors WHERE block_id = ");
    sql.bind(Id(block_id))
        .push_bind(" AND version > ", PENDING)
        .push_bind(" AND version <= ", through);
    scope.execute(sql).await?;
    Ok(true)
}

/// Forgets the oldest versions of a card until the remaining versions' `body_bytes` sum
/// to at most the store's byte budget, always keeping `newest` (even if it alone exceeds
/// the budget). Returns whether any version was forgotten.
async fn forget_over_budget(
    scope: &mut Scope<'_>,
    block_id: &str,
    newest: i64,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT version, body_bytes FROM body_versions WHERE block_id = ");
    sql.bind(Id(block_id)).push(" ORDER BY version DESC");
    let versions: Vec<(i64, i64)> = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| Ok((row.int(0)?, row.int(1)?)))
        .collect::<Result<_, StoreError>>()?;
    match budget_cutoff(&versions, newest, scope.body_versions_max_bytes()) {
        Some(through) => forget_past(scope, block_id, through).await,
        None => Ok(false),
    }
}

/// The highest version number to forget (inclusive) so that the remaining versions'
/// non-negative byte sizes sum to at most `max_bytes`, given `(version, body_bytes)`
/// ordered newest first. `newest` is always kept, even when it alone exceeds `max_bytes`.
fn budget_cutoff(versions_desc: &[(i64, i64)], newest: i64, max_bytes: u64) -> Option<i64> {
    let mut kept_bytes = 0_u128;
    let limit = u128::from(max_bytes);
    for &(version, bytes) in versions_desc {
        let size = u128::from(u64::try_from(bytes).unwrap_or(0));
        kept_bytes += size;
        if version < newest && kept_bytes > limit {
            return Some(version);
        }
    }
    None
}

/// Puts down who wrote a card's body since its newest version, besides those put down
/// already.
pub(crate) async fn note_editors(
    scope: &mut Scope<'_>,
    block_id: &str,
    user_ids: &[&str],
) -> Result<(), StoreError> {
    if user_ids.is_empty() {
        return Ok(());
    }
    let mut sql =
        scope.sql("INSERT INTO body_version_editors (block_id, version, user_id) VALUES ");
    for (index, user_id) in user_ids.iter().enumerate() {
        sql.push(if index == 0 { "(" } else { ", (" })
            .bind(Id(block_id))
            .push_bind(", ", PENDING)
            .push_bind(", ", Id(user_id))
            .push(")");
    }
    sql.push(" ON CONFLICT DO NOTHING");
    scope.execute(sql).await?;
    Ok(())
}

/// Who is put down as having written a card's body since its newest version.
pub(crate) async fn unkept_editors(
    scope: &mut Scope<'_>,
    block_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql("SELECT user_id FROM body_version_editors WHERE block_id = ");
    sql.bind(Id(block_id))
        .push_bind(" AND version = ", PENDING)
        .push(" ORDER BY user_id");
    scope.all(sql).await
}

/// The stored names of the files that the kept versions of a card's body show.
pub(crate) async fn files_of(
    scope: &mut Scope<'_>,
    block_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql("SELECT files FROM body_versions WHERE block_id = ");
    sql.bind(Id(block_id));
    let mut names = Vec::new();
    for row in scope.fetch_all(sql).await? {
        let files: Vec<String> =
            serde_json::from_str(&row.text(0)?).map_err(|error| StoreError::Corrupt {
                column: "files",
                reason: error.to_string(),
            })?;
        names.extend(files);
    }
    Ok(names)
}

/// Forgets the versions of the cards that are gone for good, and who wrote them:
/// neither on a board nor in the trash of the organization of the row.
pub(crate) async fn forget_gone_blocks(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    for table in ["body_versions", "body_version_editors"] {
        let mut statement = SqlFragment::of(&[
            "DELETE FROM ",
            table,
            " WHERE NOT EXISTS (SELECT 1 FROM blocks b WHERE b.id = ",
            table,
            ".block_id",
        ]);
        statement
            .push_fragment(&scope.same_organization(&[("b", table)]))
            .push(") AND NOT EXISTS (SELECT 1 FROM deleted_blocks d WHERE d.id = ")
            .push(table)
            .push(".block_id")
            .push_fragment(&scope.same_organization(&[("d", table)]))
            .push(")");
        scope.execute(scope.sql(&statement)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use fvoci_model::BlockType;

    use super::{budget_cutoff, with_room_sql};

    #[test]
    fn with_room_sql_matches_block_type_with_room() {
        let types = BlockType::WITH_ROOM.map(|with_room| format!("'{}'", with_room.as_str()));
        assert_eq!(
            crate::db::IntoSql::into_sql_string(&with_room_sql("b.type")),
            format!("b.type IN ({})", types.join(", "))
        );
    }

    #[test]
    fn budget_cutoff_keeps_within_budget_and_never_drops_newest() {
        assert_eq!(
            budget_cutoff(&[(3, 40), (2, 30), (1, 30)], 3, 100),
            None,
            "exact fit keeps all versions"
        );
        assert_eq!(
            budget_cutoff(&[(3, 40), (2, 30), (1, 31)], 3, 100),
            Some(1),
            "one byte over drops oldest version"
        );
        assert_eq!(
            budget_cutoff(&[(3, 60), (2, 50), (1, 10)], 3, 100),
            Some(2),
            "exceeding at v2 drops v2 and older"
        );
        assert_eq!(
            budget_cutoff(&[(1, 500)], 1, 100),
            None,
            "single version exceeding budget is kept"
        );
        assert_eq!(
            budget_cutoff(&[(4, 120), (3, 0), (2, 50), (1, 0)], 4, 100),
            Some(3),
            "when newest exceeds budget, every older version is dropped"
        );
        assert_eq!(
            budget_cutoff(&[(4, 60), (3, 40), (2, 0), (1, 50)], 4, 100),
            Some(1),
            "zero-byte version stays while cumulative total fits budget"
        );
        assert_eq!(
            budget_cutoff(&[(2, i64::MAX), (1, i64::MAX)], 2, u64::MAX),
            None,
            "two i64::MAX entries fit inside u64::MAX"
        );
        assert_eq!(
            budget_cutoff(&[(3, i64::MAX), (2, i64::MAX), (1, 2)], 3, u64::MAX),
            Some(1),
            "cumulative sum exceeding u64::MAX still prunes without saturating"
        );
    }
}
