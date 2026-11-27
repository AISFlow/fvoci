//! The Y documents of the bodies people edit together, as they are stored: a state and
//! the updates taken after it (`body_docs`, `body_updates`).
//!
//! The server takes an update into a document and logs it here before anybody else is
//! sent it, folds the log into the state when it grows, and writes the document's body
//! to its block a little later ([`Store::project_body`]). A document is its block's:
//! it stays while the block is in the trash and goes with it when it is gone for good
//! ([`forget_gone_blocks`]). What a document's bytes are is the server's to know: the
//! store keeps them.

use fvoci_model::{BlockPatch, Body, BodyChange};

use crate::Store;
use crate::block_derived::{self, Before};
use crate::blocks;
use crate::body_versions::{self, BodyBefore, BodyWrite};
use crate::db::{Id, OptId, Param, Row, Scope, Sql, SqlFragment};
use crate::error::StoreError;
use crate::file_refs::{self, NewBodyFiles};
use crate::notification_noting::Telling;

/// Maximum updates inserted per multi-row `INSERT INTO body_updates` statement.
const UPDATE_INSERT_BATCH: usize = 300;

/// A stored document and the updates it took after its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDoc {
    /// The document as a version 1 update of all of it.
    pub state: Vec<u8>,
    /// The last update the state holds; 0 for none.
    pub state_seq: i64,
    /// The last update logged: the next one is numbered after it.
    pub log_end: i64,
    /// The last update whose body the block holds.
    pub projected_seq: i64,
    /// The digest of the block's body as it was when the document's body was last the
    /// same: a body that is not that any more was written without the document.
    pub body_digest: Vec<u8>,
    /// Which making of the document it is: 1 for the first, one more each time it is
    /// made again ([`Store::remake_body_doc`]).
    pub epoch: i64,
    /// The user whose editor sent the last update logged; empty for none.
    pub last_writer: String,
    /// The updates after `state_seq`, in the order they were taken.
    pub updates: Vec<LoggedUpdate>,
    /// Who is put down as having written the body since the block's newest version
    /// ([`Store::append_body_updates`]): none once a version holds what they wrote.
    pub unkept_editors: Vec<String>,
}

/// What [`Store::project_body`] writes to the block of a document.
#[derive(Debug, Clone, PartialEq)]
pub enum Projection {
    /// The block holds the body of the document already: it is not patched.
    Held,
    /// The block takes `body`, if it was not changed since it was read: `read_at` is
    /// its `updated_at` then. (A restore from the trash in the same millisecond as the
    /// read, and the import's own writing of bodies, which keeps `updated_at`, are not
    /// seen; neither writes a block whose document is open.)
    Written {
        /// The body the block takes; `None` clears it.
        body: Option<Body>,
        /// The block's `updated_at` as it was read.
        read_at: i64,
    },
}

/// The numbers of a stored document as whoever writes it read them, or left them
/// with its own last write. Every write of the document, of its log, and of its body
/// to its block names them ([`Store::append_body_updates`],
/// [`Store::fold_body_doc`], [`Store::remake_body_doc`], [`Store::project_body`]),
/// and is refused once another writer changed any of them
/// ([`StoreError::DocumentWritten`]): what was logged, folded, written to the block
/// or made again since is the other's, and is not written over or logged after
/// unseen. The other writer is a server that took the lock of the server while the
/// one before had not noticed yet that it lost it.
///
/// Each of those writes changes at least one of the numbers, and none of them ever
/// goes back: a writer whose numbers are the stored ones holds the document as it is
/// stored, however it read it ([`Store::get_body_doc`] reads the document and its
/// log one after the other). The one write that changes none is a fold up to where
/// the state already is, which stores a state repaired as the document was opened:
/// a repair only deletes, and is the same whoever makes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocAsRead {
    /// [`StoredDoc::epoch`] as it was read.
    pub epoch: i64,
    /// [`StoredDoc::state_seq`] as it was read.
    pub state_seq: i64,
    /// [`StoredDoc::log_end`] as it was read.
    pub log_end: i64,
    /// [`StoredDoc::projected_seq`] as it was read.
    pub projected_seq: i64,
}

impl StoredDoc {
    /// The numbers of the document as it is read here, for a write of it.
    #[must_use]
    pub fn as_read(&self) -> DocAsRead {
        DocAsRead {
            epoch: self.epoch,
            state_seq: self.state_seq,
            log_end: self.log_end,
            projected_seq: self.projected_seq,
        }
    }
}

/// An update a document took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedUpdate {
    /// Its number: one after the update before it.
    pub seq: i64,
    /// The update, version 1.
    pub bytes: Vec<u8>,
    /// The user whose editor sent it; empty for the server's own.
    pub written_by: String,
    /// When it was taken, in milliseconds since the epoch.
    pub written_at: i64,
}

impl Store {
    /// The board a card is on, or the wiki a page is in; `None` for no block of that
    /// identifier, and for a block whose body has no room ([`fvoci_model::BlockType::has_room`]).
    /// Nothing else of the block is read: who asks whether somebody may come to its
    /// body is answered in the same time whatever the body is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn board_of_card(&self, card_id: &str) -> Result<Option<String>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT type, container_id FROM blocks WHERE id = ");
            sql.bind(Id(card_id));
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(None);
            };
            let has_room = crate::blocks::block_type_column(&row, 0, "type")?.has_room();
            Ok(if has_room { Some(row.id(1)?) } else { None })
        })
        .await
    }

    /// The stored document of a block, with the updates it took after its state;
    /// `None` if the block has none. Read in two statements outside a transaction: the
    /// server writes a block's document only while it holds the block's place, as the
    /// reader does (the purge of the trash aside, which removes it whole).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn get_body_doc(&self, block_id: &str) -> Result<Option<StoredDoc>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT state, state_seq, log_end, projected_seq, body_digest, epoch, \
                 last_writer FROM body_docs WHERE block_id = ",
            );
            sql.bind(Id(block_id));
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(None);
            };
            let state_seq = row.int(1)?;
            let mut sql = scope.sql(
                "SELECT seq, bytes, written_by, written_at FROM body_updates WHERE block_id = ",
            );
            sql.bind(Id(block_id))
                .push_bind(" AND seq > ", state_seq)
                .push(" ORDER BY seq");
            let updates = scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(logged_update)
                .collect::<Result<_, _>>()?;
            Ok(Some(StoredDoc {
                state: row.bytes(0)?,
                state_seq,
                log_end: row.int(2)?,
                projected_seq: row.int(3)?,
                body_digest: row.bytes(4)?,
                epoch: row.int(5)?,
                last_writer: row.id(6)?,
                updates,
                unkept_editors: body_versions::unkept_editors(scope, block_id).await?,
            }))
        })
        .await
    }

    /// Stores the first document of a block, made from its body, whose digest is
    /// `body_digest`: the block holds its body.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the block is not there (or not in reach),
    /// and [`StoreError::Database`] if it has a document already.
    pub async fn create_body_doc(
        &self,
        block_id: &str,
        state: &[u8],
        body_digest: &[u8],
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            blocks::get_block(scope, block_id).await?;
            let now = scope.now_millis();
            let mut sql = scope.sql(
                "INSERT INTO body_docs (block_id, state, state_seq, log_end, projected_seq, \
                 body_digest, epoch, created_at, updated_at) VALUES (",
            );
            sql.bind(Id(block_id))
                .push_bind(", ", Param::Bytes(state.to_vec()))
                .push(", 0, 0, 0")
                .push_bind(", ", Param::Bytes(body_digest.to_vec()))
                .push(", 1")
                .push_bind(", ", now)
                .push_bind(", ", now)
                .push(")");
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Logs updates a block's document took, in order and all or none, and gives
    /// their numbers. Each comes with the user whose editor sent it; empty for the
    /// server's own. `files` are the files the images and file blocks of the
    /// document's body name with them: those the block does not show yet are asked of its board and held as
    /// a write of its body holds them ([`crate::NewBodyFiles::Recorded`]), and the block
    /// shows them from now on, so that the files are not collected before the body is
    /// written to it. `new_editors` are those of the senders who are not yet put down
    /// as having written the body since its newest version
    /// ([`Store::keep_body_version`]), as far as the caller knows: putting one down
    /// again changes nothing. With [`BodyBefore::Keep`] the body the block holds is
    /// kept as a version first, by those put down before.
    ///
    /// Only the document as it was read (`as_read`) logs them: one that was written
    /// since holds what the writer of these never took, and nothing of this call is
    /// written.
    ///
    /// A handle of an agent ([`Store::by_agent`]) logs the updates of a user as sent
    /// through its token, and puts the agent down as having written the body since its
    /// newest version: the next version kept is the agent's
    /// ([`Store::keep_body_version`]), whoever writes the body to the block.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the block, or its document, is not there,
    /// [`StoreError::DocumentWritten`] if the document is not as it was read, and
    /// [`StoreError::InvalidBlock`] if its board has no file one of them names.
    pub async fn append_body_updates(
        &self,
        block_id: &str,
        updates: &[(&[u8], &str)],
        as_read: DocAsRead,
        files: &[String],
        new_editors: &[&str],
        body_before: BodyBefore,
    ) -> Result<Vec<i64>, StoreError> {
        self.transaction(async |scope| {
            if body_before == BodyBefore::Keep {
                body_versions::keep_held(scope, block_id).await?;
            }
            if !files.is_empty() {
                file_refs::show_body_files(scope, block_id, files).await?;
            }
            body_versions::note_editors(scope, block_id, new_editors).await?;
            if updates.is_empty() {
                return Ok(Vec::new());
            }
            let now = scope.now_millis();
            let count = i64::try_from(updates.len()).unwrap_or(i64::MAX);
            let last_writer = updates
                .iter()
                .rev()
                .map(|(_, written_by)| *written_by)
                .find(|written_by| !written_by.is_empty());
            let agent = scope
                .agent()
                .filter(|_| last_writer.is_some())
                .map(|agent| (agent.token_id.to_owned(), agent.token_name.to_owned()));
            let mut sql = scope.sql("UPDATE body_docs SET log_end = log_end + ");
            sql.bind(count);
            if let Some(written_by) = last_writer {
                sql.push_bind(", last_writer = ", OptId(written_by));
            }
            if let Some((token_id, token_name)) = &agent {
                sql.push_bind(", agent_token_id = ", Id(token_id))
                    .push_bind(", agent_token_name = ", token_name.as_str());
            }
            sql.push_bind(", updated_at = ", now);
            where_as_read(&mut sql, block_id, as_read);
            sql.push(" RETURNING log_end");
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Err(not_as_read(scope, block_id).await);
            };
            let new_end = row.int(0)?;
            let first_seq = new_end - count + 1;
            let seqs: Vec<i64> = (first_seq..=new_end).collect();
            for (chunk_idx, chunk) in updates.chunks(UPDATE_INSERT_BATCH).enumerate() {
                let base_seq =
                    first_seq + i64::try_from(chunk_idx * UPDATE_INSERT_BATCH).unwrap_or(0);
                let mut insert = scope.sql(
                    "INSERT INTO body_updates (block_id, seq, bytes, written_by, written_at, \
                     actor_token_id) VALUES ",
                );
                for (offset, (bytes, written_by)) in chunk.iter().enumerate() {
                    let seq = base_seq + i64::try_from(offset).unwrap_or(0);
                    // The server's own update (a repair) is nobody's, and no agent's.
                    let token_id = match &agent {
                        Some((token_id, _)) if !written_by.is_empty() => token_id.as_str(),
                        _ => "",
                    };
                    insert
                        .push(if offset == 0 { "(" } else { ", (" })
                        .bind(Id(block_id))
                        .push_bind(", ", seq)
                        .push_bind(", ", Param::Bytes(bytes.to_vec()))
                        .push_bind(", ", OptId(written_by))
                        .push_bind(", ", now)
                        .push_bind(", ", OptId(token_id))
                        .push(")");
                }
                scope.execute(insert).await?;
            }
            Ok(seqs)
        })
        .await
    }

    /// Folds the updates up to `through_seq` into a block's document: `state` is the
    /// document with them, and they are removed from the log. Only the document as it
    /// was read (`as_read`) is folded: one that was written since is left as it is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the block has no document, and
    /// [`StoreError::DocumentWritten`] if the document is not as it was read.
    pub async fn fold_body_doc(
        &self,
        block_id: &str,
        state: &[u8],
        through_seq: i64,
        as_read: DocAsRead,
    ) -> Result<(), StoreError> {
        debug_assert!(
            through_seq >= as_read.state_seq,
            "a fold through {through_seq} of a state that holds {} already",
            as_read.state_seq
        );
        self.transaction(async |scope| {
            let mut sql = scope.sql("UPDATE body_docs SET state = ");
            sql.bind(Param::Bytes(state.to_vec()))
                .push_bind(", state_seq = ", through_seq)
                .push_bind(", updated_at = ", scope.now_millis());
            where_as_read(&mut sql, block_id, as_read);
            if scope.execute(sql).await? == 0 {
                return Err(not_as_read(scope, block_id).await);
            }
            let mut sql = scope.sql("DELETE FROM body_updates WHERE block_id = ");
            sql.bind(Id(block_id))
                .push_bind(" AND seq <= ", through_seq);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Replaces a block's document with one made again from the body the block holds,
    /// whose digest is `body_digest`: the log is emptied, the numbers of updates go on
    /// after the last one, and its epoch, which this gives, is one more. Only the
    /// document as it was read (`as_read`) is replaced: the log of one that was
    /// written since may hold what its reader never saw, and stays.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the block has no document, and
    /// [`StoreError::DocumentWritten`] if the document is not as it was read.
    pub async fn remake_body_doc(
        &self,
        block_id: &str,
        state: &[u8],
        body_digest: &[u8],
        as_read: DocAsRead,
    ) -> Result<i64, StoreError> {
        self.transaction(async |scope| {
            let mut sql = scope.sql("UPDATE body_docs SET state = ");
            sql.bind(Param::Bytes(state.to_vec()))
                .push(", state_seq = log_end, projected_seq = log_end, epoch = epoch + 1")
                .push_bind(", body_digest = ", Param::Bytes(body_digest.to_vec()))
                .push_bind(", updated_at = ", scope.now_millis());
            where_as_read(&mut sql, block_id, as_read);
            sql.push(" RETURNING epoch");
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Err(not_as_read(scope, block_id).await);
            };
            let epoch = row.int(0)?;
            // The row is held from the update above: nothing is logged between it and
            // this, and the log is the one that was read.
            let mut sql = scope.sql("DELETE FROM body_updates WHERE block_id = ");
            sql.bind(Id(block_id));
            scope.execute(sql).await?;
            Ok(epoch)
        })
        .await
    }

    /// Writes the body of a block's document to the block, as the document was after
    /// the update `through_seq`, as `projection` says: the block is patched by
    /// `user_id` as any change of its body is (its plain text, task counts and the
    /// files it shows with it), and the document is marked as written up to there, with
    /// `body_digest` the digest of the body. A block that holds the body already
    /// ([`Projection::Held`]) is not patched.
    ///
    /// Only the body of the document as it was read (`as_read`) is written: the block
    /// is left as it is if the document was written since, also by one whose body is
    /// the block's already and changed nothing of the block.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the block, or its document, is not there,
    /// [`StoreError::Stale`] if the block changed since it was read,
    /// [`StoreError::DocumentWritten`] if the document is not as it was read, and the
    /// errors of a patch of the block.
    pub async fn project_body(
        &self,
        block_id: &str,
        projection: Projection,
        through_seq: i64,
        body_digest: &[u8],
        user_id: &str,
        as_read: DocAsRead,
    ) -> Result<(), StoreError> {
        debug_assert!(
            through_seq >= as_read.projected_seq,
            "a body as of {through_seq} for a block that holds the one of {} already",
            as_read.projected_seq
        );
        self.transaction(async |scope| {
            match projection {
                // What the block shows is put down again all the same, in place of the
                // images the log showed meanwhile. Nothing else that follows from the
                // block changes: it is as it was stored.
                Projection::Held => {
                    let block = blocks::get_block(scope, block_id).await?;
                    block_derived::refresh(
                        scope,
                        &block,
                        Before::Unchanged,
                        NewBodyFiles::Unchecked,
                    )
                    .await?;
                }
                Projection::Written { body, read_at } => {
                    let patch = BlockPatch {
                        body: body.map_or(BodyChange::Clear, BodyChange::Set),
                        read_at: Some(read_at),
                        ..BlockPatch::default()
                    };
                    // No version: the server keeps one when it is due.
                    let projected = BodyWrite::Projected;
                    // Its writer is the last of those who typed it: who typed a name
                    // it newly holds is read off the updates it did not hold yet.
                    let telling = Telling::of_typed(scope, as_read.projected_seq);
                    blocks::patch_block(scope, block_id, &patch, user_id, projected, telling)
                        .await?;
                }
            }
            // The document's row last, as an update takes it; a document that is not
            // as it was read fails the transaction, and the write of the block with it.
            let mut sql = scope.sql("UPDATE body_docs SET projected_seq = ");
            sql.bind(through_seq)
                .push_bind(", body_digest = ", Param::Bytes(body_digest.to_vec()));
            where_as_read(&mut sql, block_id, as_read);
            if scope.execute(sql).await? == 0 {
                return Err(not_as_read(scope, block_id).await);
            }
            Ok(())
        })
        .await
    }
}

impl Store {
    /// Whether a block has a document whose log holds updates the block's body does not.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn has_unwritten_body(&self, block_id: &str) -> Result<bool, StoreError> {
        self.with_conn(async |scope| {
            let mut sql =
                scope.sql("SELECT 1 FROM body_docs WHERE projected_seq < log_end AND block_id = ");
            sql.bind(Id(block_id));
            Ok(scope.fetch_optional(sql).await?.is_some())
        })
        .await
    }

    /// Those of `block_ids` that have a document whose log holds updates the block's
    /// body does not: [`Self::has_unwritten_body`] for many blocks at once.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn with_unwritten_bodies(
        &self,
        block_ids: &[String],
    ) -> Result<Vec<String>, StoreError> {
        /// Identifiers bound in one query: far below the bound parameters either
        /// database takes.
        const CHUNK: usize = 500;
        self.with_conn(async |scope| {
            let mut found = Vec::new();
            for chunk in block_ids.chunks(CHUNK) {
                let mut sql = scope.sql(
                    "SELECT block_id FROM body_docs WHERE projected_seq < log_end AND block_id IN (",
                );
                sql.bind_list(chunk.iter().map(|id| Id(id))).push(")");
                for row in scope.fetch_all(sql).await? {
                    found.push(row.id(0)?);
                }
            }
            Ok(found)
        })
        .await
    }

    /// The documents whose block does not hold their body yet, or is a card or a page that
    /// holds a body its editors wrote since its newest version, with the organization of each: what a
    /// server that ended before it wrote and kept them has to. Blocks in the trash
    /// are left out; theirs is written when they are opened again.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn unfinished_body_docs(&self) -> Result<Vec<(String, String)>, StoreError> {
        self.body_docs_where(|scope, sql| {
            sql.push(" WHERE d.projected_seq < d.log_end OR (")
                .push(&body_versions::with_room_sql("b.type"))
                .push_bind(
                    " AND EXISTS (SELECT 1 FROM body_version_editors e \
                     WHERE e.block_id = d.block_id AND e.version = ",
                    body_versions::PENDING,
                )
                .push(&scope.same_organization(&[("e", "d")]))
                .push("))");
        })
        .await
    }

    /// How many updates a block's log holds, whether or not its state holds them: for
    /// the tests of the folds, which remove them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    #[cfg(feature = "test-util")]
    pub async fn logged_body_updates(&self, block_id: &str) -> Result<i64, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT COUNT(*) FROM body_updates WHERE block_id = ");
            sql.bind(Id(block_id));
            match scope.fetch_optional(sql).await? {
                Some(row) => row.int(0),
                None => Ok(0),
            }
        })
        .await
    }

    /// The documents of cards and pages whose log holds updates their state does not,
    /// with the organization of each: what a server that ended before it folded them
    /// leaves. Blocks in the trash are left out; theirs is folded when they are opened
    /// again and closed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn unfolded_body_docs(&self) -> Result<Vec<(String, String)>, StoreError> {
        self.body_docs_where(|_, sql| {
            // Of a block that is a card or a page no more, nothing is opened.
            sql.push(" WHERE d.state_seq < d.log_end AND ")
                .push(&body_versions::with_room_sql("b.type"));
        })
        .await
    }

    /// The documents of the blocks that are not in the trash, of the condition
    /// `condition` appends for a scope (on `d`, the document, and `b`, its block), with
    /// the organization of each, by block.
    async fn body_docs_where(
        &self,
        condition: impl Fn(&Scope<'_>, &mut Sql) + Send + Sync,
    ) -> Result<Vec<(String, String)>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT d.block_id, w.org_id FROM body_docs d JOIN blocks b ON b.id = d.block_id",
            );
            sql.push(&scope.same_organization(&[("b", "d")]))
                .push(" JOIN containers o ON o.id = b.container_id")
                .push(&scope.same_organization(&[("o", "b")]))
                .push(" JOIN workspaces w ON w.id = o.workspace_id")
                .push(&scope.same_organization(&[("w", "o")]));
            condition(scope, &mut sql);
            sql.push(" ORDER BY d.block_id");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| Ok((row.id(0)?, row.id(1)?)))
                .collect()
        })
        .await
    }
}

fn logged_update(row: &Row) -> Result<LoggedUpdate, StoreError> {
    Ok(LoggedUpdate {
        seq: row.int(0)?,
        bytes: row.bytes(1)?,
        written_by: row.id(2)?,
        written_at: row.int(3)?,
    })
}

fn no_doc(block_id: &str) -> StoreError {
    StoreError::not_found(format_args!("the document of block ID={block_id}"))
}

/// Ends a statement that writes a block's document with the condition every such
/// write has: the document of the block, with its numbers as they were read. The
/// check and the write are one statement, so nothing is written in between.
fn where_as_read(sql: &mut Sql, block_id: &str, as_read: DocAsRead) {
    sql.push_bind(" WHERE block_id = ", Id(block_id))
        .push_bind(" AND epoch = ", as_read.epoch)
        .push_bind(" AND state_seq = ", as_read.state_seq)
        .push_bind(" AND log_end = ", as_read.log_end)
        .push_bind(" AND projected_seq = ", as_read.projected_seq);
}

/// Why a write of a block's document as it was read wrote nothing: the block has no
/// document, or the document is not as it was read any more.
async fn not_as_read(scope: &mut Scope<'_>, block_id: &str) -> StoreError {
    let mut sql = scope.sql("SELECT 1 FROM body_docs WHERE block_id = ");
    sql.bind(Id(block_id));
    match scope.fetch_optional(sql).await {
        Ok(Some(_)) => StoreError::DocumentWritten {
            block_id: block_id.to_owned(),
        },
        Ok(None) => no_doc(block_id),
        Err(error) => error,
    }
}

/// The updates the log of a block's document holds past `seq`, the oldest first, each
/// with the user whose editor sent it: who typed what the block's body does not hold
/// yet. The server's own updates are not among them; and those that were folded into
/// the document's state meanwhile are in no log any more.
pub(crate) async fn updates_since(
    scope: &mut Scope<'_>,
    block_id: &str,
    seq: i64,
) -> Result<Vec<TypedUpdate>, StoreError> {
    // A token is no organization's: it is read by its identifier alone.
    let mut sql = scope.sql(
        "SELECT u.written_by, u.bytes, t.id, t.name, t.notify_self FROM body_updates AS u \
         LEFT JOIN api_tokens AS t ON t.id = u.actor_token_id WHERE u.block_id = ",
    );
    sql.bind(Id(block_id))
        .push_bind(" AND u.seq > ", seq)
        .push(" AND u.written_by IS NOT NULL ORDER BY u.seq");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            let token_id = row.id(2)?;
            let agent = if token_id.is_empty() {
                None
            } else {
                Some(TypingAgent {
                    token_id,
                    token_name: row.text(3)?,
                    notify_self: row.bool(4)?,
                })
            };
            Ok(TypedUpdate {
                written_by: row.id(0)?,
                bytes: row.bytes(1)?,
                agent,
            })
        })
        .collect()
}

/// An update of a document that its body did not hold yet ([`updates_since`]).
#[derive(Debug, Clone)]
pub(crate) struct TypedUpdate {
    /// Who sent it.
    pub(crate) written_by: String,
    /// The update.
    pub(crate) bytes: Vec<u8>,
    /// The agent that sent it through a token of `written_by`, if one did.
    pub(crate) agent: Option<TypingAgent>,
}

/// The agent of a [`TypedUpdate`], as its token is now.
#[derive(Debug, Clone)]
pub(crate) struct TypingAgent {
    pub(crate) token_id: String,
    pub(crate) token_name: String,
    pub(crate) notify_self: bool,
}

impl TypingAgent {
    pub(crate) fn by(&self) -> crate::ByAgent<'_> {
        crate::ByAgent {
            token_id: &self.token_id,
            token_name: &self.token_name,
            notify_self: self.notify_self,
        }
    }
}

/// Removes the documents whose blocks are gone for good: neither there nor in the
/// trash of their organization. Their logs go with them.
pub(crate) async fn forget_gone_blocks(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut gone = SqlFragment::of(&[
        " WHERE NOT EXISTS (SELECT 1 FROM blocks b WHERE b.id = body_docs.block_id",
    ]);
    gone.push_fragment(&scope.same_organization(&[("b", "body_docs")]))
        .push(") AND NOT EXISTS (SELECT 1 FROM deleted_blocks d WHERE d.id = body_docs.block_id")
        .push_fragment(&scope.same_organization(&[("d", "body_docs")]))
        .push(")");

    let mut updates =
        SqlFragment::of(&["DELETE FROM body_updates WHERE EXISTS (SELECT 1 FROM body_docs"]);
    updates
        .push_fragment(&gone)
        .push(" AND body_docs.block_id = body_updates.block_id")
        .push_fragment(&scope.same_organization(&[("body_docs", "body_updates")]))
        .push(")");
    scope.execute(scope.sql(&updates)).await?;

    let mut docs = SqlFragment::of(&["DELETE FROM body_docs"]);
    docs.push_fragment(&gone);
    scope.execute(scope.sql(&docs)).await?;
    Ok(())
}

/// Whether `block_id` has collaborative updates in `body_docs` that have not yet been
/// projected into `blocks.body` (`projected_seq < log_end`).
pub(crate) async fn has_unwritten_body(
    scope: &mut Scope<'_>,
    block_id: &str,
) -> Result<bool, StoreError> {
    let mut sql =
        scope.sql("SELECT 1 FROM body_docs WHERE projected_seq < log_end AND block_id = ");
    sql.bind(Id(block_id));
    Ok(scope.fetch_optional(sql).await?.is_some())
}
